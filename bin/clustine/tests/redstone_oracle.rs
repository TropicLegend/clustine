//! Comparisons with the official server of redstone's timing and of what a client is
//! sent of it (ADR-0026, step R8): the clock of two observers facing each other and a
//! torch clock that burns out, a lamp behind wire and behind a repeater, pistons pushing
//! and pulling, and doors, a trapdoor and a gate opened by a lever and by hand, with
//! their sounds.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them, with every
//! case's timelines printed, on a quiet machine, with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test redstone_oracle -- --ignored --nocapture --test-threads 1
//! ```
//!
//! A case is built by a builder from plain clicks, as a player would, and then run as
//! stages: a click (a lever flipped, the block that closes a circuit) and the state that
//! shows it has done its work. A second bot, the watcher, hovers above the case and
//! records, as they arrive, the block changes, block events, sounds and level events in
//! the case's box; it reads in a task of its own, so that when something came is when it
//! was sent. A stage's timeline is what the watcher was sent from the stage's click on,
//! each thing at its tick counted from the first, a tick being the twentieth of a second
//! between them rounded. The two servers' timelines are to hold the same things in the
//! same order, each within a tick of the other's time. Within a packet of a section's
//! changes the blocks are sorted, since a client sets them all at once; a sound's pitch,
//! which both servers draw at random for most sounds, is shown and not compared.
//!
//! What a stage waits for is a state, not a time, as everywhere else. Once it is
//! reached, the watcher goes on recording for a while, which is no wait for the server
//! but the window in which what comes after it is seen; a torch clock's is long enough
//! to see whether a burnt-out torch lights again.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clustine_botswarm::{Bot, Heard, Oracle};
use clustine_data::{BlockState, SOUND_EVENTS, items};
use clustine_protocol::packets::play::face::{EAST, NORTH, SOUTH, TOP as UP, WEST};
use tokio::sync::Mutex;
use tokio::time::Instant;

use common::start;

/// How long a bot waits for what it should be sent soon: an acknowledgement, a block
/// it placed, the chunks of a case, the state a stage ends in.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long the builder waits to be shown what a step of the setup placed.
const SETUP_PATIENCE: Duration = Duration::from_secs(5);

/// The window after a stage's state in which the watcher goes on recording: twenty
/// ticks, more than anything here takes after the state it ends in.
const AFTER: Duration = Duration::from_secs(1);

/// The window after a torch clock has burnt out: 240 ticks, beyond the 160 a burnt-out
/// torch waits before it tries to light again and short of a second burnout after that.
const AFTER_BURNOUT: Duration = Duration::from_secs(12);

/// A server tick, by which arrivals are counted.
const TICK: f64 = 0.05;

/// The hotbar slot the builder holds its item in, and one it never fills.
const HELD_SLOT: i16 = 0;
const EMPTY_SLOT: i16 = 8;

/// The grid of cases, as in `placing_oracle.rs`: clear of where players join on either
/// server, within the chunks at x 0 to 3 (one region where `CLUSTINE_TEST_PINS=0,4`
/// divides the world), and far enough apart that no case's box holds another's blocks.
const FIRST_X: i32 = 14;
const FIRST_Z: i32 = -42;
const SPACING: i32 = 10;
const COLUMNS: usize = 4;

/// How far a case's box reaches from its origin: three blocks each way across, two
/// below the first block above the ground and three above it.
const BOX_ACROSS: i32 = 3;
const BOX_BELOW: i32 = 2;
const BOX_ABOVE: i32 = 3;

/// How high above the ground the builder flies between cases, and the watcher hovers.
const CRUISE: f64 = 3.0;
const HOVER: f64 = 6.0;

/// A position relative to a case's origin: x and z as in the world, and y from the
/// first block above the ground.
type Rel = (i32, i32, i32);

/// A yaw and a pitch in degrees: a yaw of 0 looks south, 90 west, 180 north and 270
/// east.
type Look = (f32, f32);

const LOOKS_NORTH: Look = (180.0, 0.0);
const LOOKS_EAST: Look = (270.0, 0.0);
const LOOKS_WEST: Look = (90.0, 0.0);

/// Where the builder stands for a case: three blocks south of the origin, on the ground.
const STAND: (f64, f64, f64) = (0.5, 0.0, 3.5);

const O: Rel = (0, 0, 0);

#[derive(Debug, Clone, Copy)]
enum Act {
    /// The use button on a face of a block, in the middle of the face.
    Click { at: Rel, face: u8 },
    /// Breaking a block, as a player in creative mode does.
    Dig(Rel),
}

#[derive(Debug, Clone, Copy)]
struct Step {
    /// The item held, or none.
    held: Option<i32>,
    look: Look,
    act: Act,
}

impl Step {
    fn look(mut self, look: Look) -> Self {
        self.look = look;
        self
    }
}

/// A click with `item` on the `face` of the block `at`, looking north.
fn click(item: i32, at: Rel, face: u8) -> Step {
    Step {
        held: Some(item),
        look: LOOKS_NORTH,
        act: Act::Click { at, face },
    }
}

/// A click with `item` on the top of the block below `at`, which places into `at`.
fn put(item: i32, at: Rel) -> Step {
    click(item, (at.0, at.1 - 1, at.2), UP)
}

/// A click with an empty hand on the top of the block `at`: a lever flipped, a door
/// opened.
fn bare(at: Rel) -> Step {
    Step {
        held: None,
        look: LOOKS_NORTH,
        act: Act::Click { at, face: UP },
    }
}

fn dig(at: Rel) -> Step {
    Step {
        held: None,
        look: LOOKS_NORTH,
        act: Act::Dig(at),
    }
}

/// What ends a stage, as the watcher is shown it.
#[derive(Debug, Clone, Copy)]
enum Until {
    /// The block at a position has this state.
    Shown(Rel, &'static str),
    /// The block at a position has changed this many times since the stage's click.
    Changes(Rel, usize),
    /// This level event has been sent since the stage's click.
    LevelEvent(i32),
}

#[derive(Debug, Clone)]
struct Stage {
    step: Step,
    until: Until,
    /// How long the watcher goes on recording once the stage's state is reached.
    after: Duration,
    /// Whether the stage's timeline is compared, or the stage only stops what the case
    /// set going.
    compared: bool,
    /// Whether the timeline is cut at the thing that ended the stage: for a clock, whose
    /// ticks go on into the window after.
    cut: bool,
}

fn stage(step: Step, until: Until) -> Stage {
    Stage {
        step,
        until,
        after: AFTER,
        compared: true,
        cut: false,
    }
}

impl Stage {
    fn after(mut self, after: Duration) -> Self {
        self.after = after;
        self
    }

    fn cut(mut self) -> Self {
        (self.cut, self.after) = (true, Duration::ZERO);
        self
    }

    fn uncompared(mut self) -> Self {
        self.compared = false;
        self
    }
}

struct Case {
    name: &'static str,
    /// What is built first; each step is waited for until the builder is shown it.
    setup: Vec<Step>,
    stages: Vec<Stage>,
}

fn case(name: &'static str, setup: Vec<Step>, stages: Vec<Stage>) -> Case {
    Case {
        name,
        setup,
        stages,
    }
}

fn state(text: &str) -> i32 {
    let state = BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a state"));
    i32::from(state.0)
}

/// A lever on the ground at `at`, beside what it powers.
fn lever(at: Rel) -> Step {
    put(items::LEVER, at)
}

fn cases() -> Vec<Case> {
    use items::*;
    let lever_west = (-1, 0, 0);
    let lever_east = (1, 0, 0);
    let flip = |at| bare(at);
    vec![
        // Two observers facing each other: placing the second sets the first going, and
        // each then pulses as the other does. Section 6 of ADR-0026 has them pulse
        // every six ticks by the order of their ticks.
        case(
            "two observers facing each other",
            vec![put(OBSERVER, O).look(LOOKS_EAST)],
            vec![
                stage(
                    put(OBSERVER, (1, 0, 0)).look(LOOKS_WEST),
                    Until::Changes(O, 24),
                )
                .cut(),
                stage(dig((1, 0, 0)), Until::Shown((1, 0, 0), "minecraft:air"))
                    .after(Duration::from_secs(2))
                    .uncompared(),
            ],
        ),
        // A torch on the east side of a stone, which powers the stone above it, which
        // powers the wire on the first stone, which powers that stone and puts the
        // torch out: a torch clock, which burns out after eight times in sixty ticks.
        case(
            "a torch clock",
            vec![
                put(STONE, O),
                put(STONE, (2, 0, 0)),
                put(STONE, (2, 1, 0)),
                click(STONE, (2, 1, 0), WEST),
                put(REDSTONE, (0, 1, 0)),
            ],
            vec![
                stage(click(REDSTONE_TORCH, O, EAST), Until::LevelEvent(1502)).after(AFTER_BURNOUT),
                stage(dig((1, 0, 0)), Until::Shown((1, 0, 0), "minecraft:air")).uncompared(),
            ],
        ),
        // A lamp lights at once and goes out four ticks after its power goes.
        case(
            "a lamp behind wire",
            vec![
                put(REDSTONE_LAMP, (1, 0, 0)),
                lever(lever_west),
                put(REDSTONE, O),
            ],
            vec![
                stage(
                    flip(lever_west),
                    Until::Shown((1, 0, 0), "minecraft:redstone_lamp[lit=true]"),
                ),
                stage(
                    flip(lever_west),
                    Until::Shown((1, 0, 0), "minecraft:redstone_lamp[lit=false]"),
                ),
            ],
        ),
        // A repeater, placed looking east, between a lever and a lamp.
        case(
            "a lamp behind a repeater",
            vec![
                put(REDSTONE_LAMP, (1, 0, 0)),
                lever(lever_west),
                put(REPEATER, O).look(LOOKS_EAST),
            ],
            vec![
                stage(
                    flip(lever_west),
                    Until::Shown((1, 0, 0), "minecraft:redstone_lamp[lit=true]"),
                ),
                stage(
                    flip(lever_west),
                    Until::Shown((1, 0, 0), "minecraft:redstone_lamp[lit=false]"),
                ),
            ],
        ),
        // A piston placed looking north faces south, towards the builder, and pushes the
        // stone before it a block; unpowered, it leaves it there.
        case(
            "a piston",
            vec![put(STONE, (0, 0, 1)), put(PISTON, O), lever(lever_west)],
            vec![
                stage(flip(lever_west), Until::Shown((0, 0, 2), "minecraft:stone")),
                stage(
                    flip(lever_west),
                    Until::Shown(O, "minecraft:piston[extended=false,facing=south]"),
                ),
            ],
        ),
        // A sticky piston pulls it back.
        case(
            "a sticky piston",
            vec![
                put(STONE, (0, 0, 1)),
                put(STICKY_PISTON, O),
                lever(lever_west),
            ],
            vec![
                stage(flip(lever_west), Until::Shown((0, 0, 2), "minecraft:stone")),
                stage(flip(lever_west), Until::Shown((0, 0, 1), "minecraft:stone")),
            ],
        ),
        // Doors and the like, with the sound each makes.
        case(
            "an oak door opened by hand",
            vec![put(OAK_DOOR, O)],
            vec![
                stage(bare_side(O, SOUTH), Until::Changes(O, 1)),
                stage(bare_side(O, SOUTH), Until::Changes(O, 1)),
            ],
        ),
        case(
            "an oak door opened by a lever",
            vec![put(OAK_DOOR, O), lever(lever_east)],
            vec![
                stage(flip(lever_east), Until::Changes(O, 1)),
                stage(flip(lever_east), Until::Changes(O, 1)),
            ],
        ),
        case(
            "an iron door opened by a lever",
            vec![put(IRON_DOOR, O), lever(lever_east)],
            vec![
                stage(flip(lever_east), Until::Changes(O, 1)),
                stage(flip(lever_east), Until::Changes(O, 1)),
            ],
        ),
        case(
            "an iron trapdoor opened by a lever",
            vec![put(IRON_TRAPDOOR, O), lever(lever_east)],
            vec![
                stage(flip(lever_east), Until::Changes(O, 1)),
                stage(flip(lever_east), Until::Changes(O, 1)),
            ],
        ),
        case(
            "an oak fence gate opened by a lever",
            vec![put(OAK_FENCE_GATE, O), lever(lever_east)],
            vec![
                stage(flip(lever_east), Until::Changes(O, 1)),
                stage(flip(lever_east), Until::Changes(O, 1)),
            ],
        ),
    ]
}

/// A click with an empty hand on a side of the block `at`.
fn bare_side(at: Rel, face: u8) -> Step {
    Step {
        held: None,
        look: LOOKS_NORTH,
        act: Act::Click { at, face },
    }
}

/// One thing in a timeline, its position relative to the case's origin.
#[derive(Debug, Clone, PartialEq)]
enum Thing {
    Block(Rel, i32),
    BlockEvent {
        at: Rel,
        action: u8,
        param: u8,
        block: i32,
    },
    /// A sound, at its place in eighths of a block from the origin's corner; the pitch
    /// is kept for showing.
    Sound {
        sound: i32,
        source: i32,
        at: Rel,
        volume: f32,
        pitch: f32,
    },
    LevelEvent {
        event: i32,
        at: Rel,
        data: i32,
    },
}

impl Thing {
    /// Whether two things are the same but for what is not compared.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Sound {
                    sound,
                    source,
                    at,
                    volume,
                    ..
                },
                Self::Sound {
                    sound: other_sound,
                    source: other_source,
                    at: other_at,
                    volume: other_volume,
                    ..
                },
            ) => (sound, source, at, volume) == (other_sound, other_source, other_at, other_volume),
            _ => self == other,
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Block(at, id) => format!("block {at:?} {}", named(*id)),
            Self::BlockEvent {
                at,
                action,
                param,
                block,
            } => format!(
                "block event {at:?} action {action} param {param} of {}",
                block_name(*block)
            ),
            Self::Sound {
                sound,
                source,
                at,
                volume,
                pitch,
            } => format!(
                "sound {} source {source} at {at:?}/8 volume {volume} (pitch {pitch:.3})",
                usize::try_from(*sound)
                    .ok()
                    .and_then(|sound| SOUND_EVENTS.get(sound))
                    .copied()
                    .unwrap_or("?")
            ),
            Self::LevelEvent { event, at, data } => {
                format!("level event {event} {at:?} data {data}")
            }
        }
    }
}

/// A timeline: each thing with its tick, counted from the first thing's arrival.
type Timeline = Vec<(i64, Thing)>;

fn named(id: i32) -> String {
    let Some(block) = u16::try_from(id)
        .ok()
        .map(BlockState)
        .filter(|s| s.block().is_some())
    else {
        return format!("the state {id}, which is no state of 26.3");
    };
    let name = block.block().map_or("?", |block| block.name);
    let properties: Vec<String> = block
        .properties()
        .map(|(property, value)| format!("{property}={value}"))
        .collect();
    if properties.is_empty() {
        name.to_owned()
    } else {
        format!("{name}[{}]", properties.join(","))
    }
}

/// A block event's block, which is a block's number in the registry, not a state.
fn block_name(block: i32) -> String {
    usize::try_from(block)
        .ok()
        .and_then(|block| clustine_data::BLOCKS.get(block))
        .map_or_else(|| format!("block {block}"), |block| block.name.to_owned())
}

/// Where a case is in the world.
#[derive(Debug, Clone, Copy)]
struct Place {
    x: i32,
    feet: i32,
    z: i32,
}

impl Place {
    fn of(index: usize, feet: i32) -> Self {
        let (row, column) = (index / COLUMNS, index % COLUMNS);
        let column = if row.is_multiple_of(2) {
            column
        } else {
            COLUMNS - 1 - column
        };
        Self {
            x: FIRST_X + SPACING * column as i32,
            feet,
            z: FIRST_Z + SPACING * row as i32,
        }
    }

    fn block(&self, at: Rel) -> (i32, i32, i32) {
        (self.x + at.0, self.feet + at.1, self.z + at.2)
    }

    fn rel(&self, (x, y, z): (i32, i32, i32)) -> Rel {
        (x - self.x, y - self.feet, z - self.z)
    }

    fn point(&self, at: (f64, f64, f64)) -> (f64, f64, f64) {
        (
            f64::from(self.x) + at.0,
            f64::from(self.feet) + at.1,
            f64::from(self.z) + at.2,
        )
    }

    fn holds(&self, at: Rel) -> bool {
        at.0.abs() <= BOX_ACROSS
            && at.2.abs() <= BOX_ACROSS
            && (-BOX_BELOW..=BOX_ABOVE).contains(&at.1)
    }

    /// The things of `heard` in the case's box, in the order they came.
    fn things(&self, heard: &[(Instant, Heard)]) -> Vec<(Instant, Thing)> {
        let mut things = Vec::new();
        for (when, heard) in heard {
            match heard {
                Heard::Blocks { blocks, .. } => {
                    for &(at, id) in blocks {
                        let at = self.rel(at);
                        if self.holds(at) {
                            things.push((*when, Thing::Block(at, id)));
                        }
                    }
                }
                &Heard::BlockEvent {
                    at,
                    action,
                    param,
                    block,
                } => {
                    let at = self.rel(at);
                    if self.holds(at) {
                        things.push((
                            *when,
                            Thing::BlockEvent {
                                at,
                                action,
                                param,
                                block,
                            },
                        ));
                    }
                }
                &Heard::Sound {
                    sound,
                    source,
                    at: (x, y, z),
                    volume,
                    pitch,
                } => {
                    let at = (x - self.x * 8, y - self.feet * 8, z - self.z * 8);
                    if self.holds((at.0 >> 3, at.1 >> 3, at.2 >> 3)) {
                        things.push((
                            *when,
                            Thing::Sound {
                                sound,
                                source,
                                at,
                                volume,
                                pitch,
                            },
                        ));
                    }
                }
                &Heard::LevelEvent { event, at, data } => {
                    let at = self.rel(at);
                    if self.holds(at) {
                        things.push((*when, Thing::LevelEvent { event, at, data }));
                    }
                }
            }
        }
        things
    }
}

/// What a run on one server came to: each case's timelines, one a stage, and what went
/// wrong on the way.
struct Ran {
    timelines: Vec<Vec<Timeline>>,
    troubles: Vec<String>,
}

/// Runs `cases` on both servers and fails, listing every difference, if they differ.
async fn compare(cases: &[Case]) {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;

    let theirs = run(oracle.address(), cases).await;
    let ours = run(&address, cases).await;
    server.stop().await;
    drop(oracle);

    let mut differences = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        println!("{index:3} {}", case.name);
        for (number, stage) in case.stages.iter().enumerate() {
            let their = theirs.timelines[index].get(number);
            let our = ours.timelines[index].get(number);
            println!("    stage {number}: {:?}", stage.step.act);
            if !stage.compared {
                println!("      not compared");
                continue;
            }
            let (Some(their), Some(our)) = (their, our) else {
                differences.push(format!(
                    "case {index}, \"{}\", stage {number}: not run on {}",
                    case.name,
                    if their.is_none() {
                        "the official server"
                    } else {
                        "Clustine"
                    }
                ));
                continue;
            };
            let rows = their.len().max(our.len());
            let mut first_difference = None;
            for row in 0..rows {
                let (t, o) = (their.get(row), our.get(row));
                let same = match (t, o) {
                    (Some((t_tick, t_thing)), Some((o_tick, o_thing))) => {
                        t_thing.same(o_thing) && (t_tick - o_tick).abs() <= 1
                    }
                    _ => false,
                };
                if !same && first_difference.is_none() {
                    first_difference = Some(row);
                }
                let show = |entry: Option<&(i64, Thing)>| {
                    entry.map_or_else(
                        || "-".to_owned(),
                        |(tick, thing)| format!("{tick:4} {}", thing.describe()),
                    )
                };
                println!(
                    "      {} {}\n        {}",
                    if same { " " } else { "≠" },
                    show(t),
                    show(o)
                );
            }
            if let Some(row) = first_difference {
                differences.push(format!(
                    "case {index}, \"{}\", stage {number}: the timelines part at their \
                     thing {row}: the official server sent {} things and Clustine {}",
                    case.name,
                    their.len(),
                    our.len()
                ));
            }
        }
    }
    let troubles: Vec<String> = theirs
        .troubles
        .iter()
        .map(|trouble| format!("on the official server: {trouble}"))
        .chain(
            ours.troubles
                .iter()
                .map(|trouble| format!("on Clustine: {trouble}")),
        )
        .collect();
    for trouble in &troubles {
        println!("{trouble}");
    }
    assert!(
        differences.is_empty() && troubles.is_empty(),
        "{} of the stages differ (each official line above its Clustine line, ≠ where \
         they part):\n  {}\n{}",
        differences.len(),
        differences.join("\n  "),
        if troubles.is_empty() {
            String::new()
        } else {
            format!(
                "and these went wrong on the way:\n  {}",
                troubles.join("\n  ")
            )
        }
    );
}

/// Runs every case on the server at `address` with a builder and a watcher.
async fn run(address: &str, cases: &[Case]) -> Ran {
    let mut troubles = Vec::new();
    let mut builder = join(address, "Builder").await;
    let feet = builder.location.1.floor() as i32;
    builder
        .empty_hotbar_slot(EMPTY_SLOT)
        .await
        .unwrap_or_else(|error| panic!("the builder's hand was not emptied: {error:#}"));
    let mut watcher = join(address, "Watcher").await;
    watcher.record_heard();
    watcher
        .set_flying(true)
        .await
        .unwrap_or_else(|error| panic!("the watcher did not fly: {error:#}"));

    // The watcher reads in a task of its own, so that what it is sent is recorded as
    // it comes; the builder takes it between packets to move it and to read it.
    let watcher = Arc::new(Mutex::new(watcher));
    let done = Arc::new(AtomicBool::new(false));
    let reading = tokio::spawn({
        let (watcher, done) = (watcher.clone(), done.clone());
        async move {
            while !done.load(Ordering::Relaxed) {
                let mut watcher = watcher.lock().await;
                // Not reaching a state is how the read ends each time.
                let _ = watcher
                    .wait_until(Duration::from_millis(20), |_| false)
                    .await;
                if let Some(ending) = &watcher.ended {
                    return Some(format!("the watcher's connection ended: {ending:?}"));
                }
                drop(watcher);
                tokio::task::yield_now().await;
            }
            None
        }
    });

    let mut timelines = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let place = Place::of(index, feet);
        match perform(&mut builder, &watcher, case, place, &mut troubles).await {
            Ok(ran) => timelines.push(ran),
            Err(error) => {
                troubles.push(format!(
                    "case {index}, \"{}\": {error:#}; the cases after it were not run",
                    case.name
                ));
                break;
            }
        }
    }
    done.store(true, Ordering::Relaxed);
    if let Some(trouble) = reading.await.expect("the watcher's task does not panic") {
        troubles.push(trouble);
    }
    Ran {
        timelines,
        troubles,
    }
}

async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name)
        .await
        .unwrap_or_else(|error| panic!("{name} could not join {address}: {error:#}"));
    bot.wait_until(PATIENCE, Bot::is_loaded)
        .await
        .unwrap_or_else(|error| panic!("{name} did not load on {address}: {error:#}"));
    bot
}

/// The builder and the watcher go to the case, the builder builds it and runs its
/// stages, and the watcher's record of each stage is taken.
async fn perform(
    builder: &mut Bot,
    watcher: &Mutex<Bot>,
    case: &Case,
    place: Place,
    troubles: &mut Vec<String>,
) -> anyhow::Result<Vec<Timeline>> {
    let (x, y, z) = place.point(STAND);
    let cruise = f64::from(place.feet) + CRUISE;
    {
        let mut watcher = watcher.lock().await;
        let (wx, wy, wz) = place.point((0.5, HOVER, 0.5));
        let from = watcher.location;
        watcher
            .fly_to((from.0, wy, from.2), (0.0, 90.0), 1.0)
            .await?;
        watcher.fly_to((wx, wy, wz), (0.0, 90.0), 1.0).await?;
        let origin = place.block(O);
        watcher
            .wait_until(PATIENCE, |bot| {
                matches!(bot.block_at(origin.0, origin.1, origin.2), Ok(Some(_)))
            })
            .await?;
    }
    let first_look = case.setup.first().map_or(LOOKS_NORTH, |step| step.look);
    let from = builder.location;
    builder
        .fly_to((from.0, cruise, from.2), first_look, 1.0)
        .await?;
    builder.fly_to((x, cruise, z), first_look, 1.0).await?;
    builder.fly_to((x, y, z), first_look, 1.0).await?;

    for (number, step) in case.setup.iter().enumerate() {
        let before = watched(builder, step, place);
        do_step(builder, step, place).await?;
        let changed = |bot: &Bot| {
            before
                .iter()
                .any(|&((x, y, z), was)| bot.block_at(x, y, z).ok().flatten() != was)
        };
        if builder.wait_until(SETUP_PATIENCE, changed).await.is_err() {
            troubles.push(format!(
                "case \"{}\", step {number} of its setup, {step:?}: the builder was not \
                 shown a change at any of {before:?}",
                case.name
            ));
        }
    }

    let mut timelines = Vec::new();
    for (number, stage) in case.stages.iter().enumerate() {
        let began = Instant::now();
        do_step(builder, &stage.step, place).await?;
        let ended = until(builder, watcher, stage.until, place, began).await;
        let Some(ended) = ended else {
            troubles.push(format!(
                "case \"{}\", stage {number}: {:?} was not reached; the stages after it \
                 were not run",
                case.name, stage.until
            ));
            timelines.push(take(watcher, place, began, None).await);
            break;
        };
        // The window after the stage's state: the builder reads what it is sent all the
        // while, so that it answers the server as a client does.
        let window = Instant::now() + stage.after;
        while Instant::now() < window {
            let _ = builder.wait_until(window - Instant::now(), |_| false).await;
        }
        let cut = stage.cut.then_some(ended);
        timelines.push(take(watcher, place, began, cut).await);
    }
    Ok(timelines)
}

/// Waits until the watcher is shown what ends a stage that began at `began`, while the
/// builder reads what it is sent. Returns the arrival of the thing that ended it, or
/// `None` if it did not come in time.
async fn until(
    builder: &mut Bot,
    watcher: &Mutex<Bot>,
    until: Until,
    place: Place,
    began: Instant,
) -> Option<Instant> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        {
            let watcher = watcher.lock().await;
            let heard = watcher.heard.as_deref().unwrap_or_default();
            let since: Vec<_> = place
                .things(heard)
                .into_iter()
                .filter(|(when, _)| *when >= began)
                .collect();
            let found = match until {
                Until::Shown(at, text) => {
                    let (x, y, z) = place.block(at);
                    let id = state(text);
                    (watcher.block_at(x, y, z).ok().flatten() == Some(id)).then(|| {
                        since
                            .iter()
                            .rev()
                            .find(|(_, thing)| *thing == Thing::Block(at, id))
                            .map_or_else(Instant::now, |(when, _)| *when)
                    })
                }
                Until::Changes(at, count) => since
                    .iter()
                    .filter(|(_, thing)| matches!(thing, Thing::Block(there, _) if *there == at))
                    .nth(count - 1)
                    .map(|(when, _)| *when),
                Until::LevelEvent(event) => since
                    .iter()
                    .find(|(_, thing)| {
                        matches!(thing, Thing::LevelEvent { event: e, .. } if *e == event)
                    })
                    .map(|(when, _)| *when),
            };
            if found.is_some() {
                return found;
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        let _ = builder
            .wait_until(Duration::from_millis(20), |_| false)
            .await;
    }
}

/// The watcher's record of a stage that began at `began`, up to `cut` if given, as a
/// timeline in ticks from its first thing.
async fn take(
    watcher: &Mutex<Bot>,
    place: Place,
    began: Instant,
    cut: Option<Instant>,
) -> Timeline {
    let watcher = watcher.lock().await;
    let heard = watcher.heard.as_deref().unwrap_or_default();
    let things: Vec<_> = place
        .things(heard)
        .into_iter()
        .filter(|(when, _)| *when >= began && cut.is_none_or(|cut| *when <= cut))
        .collect();
    let Some(&(first, _)) = things.first() else {
        return Vec::new();
    };
    things
        .into_iter()
        .map(|(when, thing)| {
            let tick = ((when - first).as_secs_f64() / TICK).round() as i64;
            (tick, thing)
        })
        .collect()
}

/// Turns the builder, takes what the step holds into its hand, does the step and waits
/// for the server to acknowledge it.
async fn do_step(builder: &mut Bot, step: &Step, place: Place) -> anyhow::Result<()> {
    let location = builder.location;
    builder.fly_to(location, step.look, 1.0).await?;
    match step.held {
        None => builder.select_slot(EMPTY_SLOT).await?,
        Some(item) => {
            builder
                .take_from_creative_inventory(HELD_SLOT, item)
                .await?;
            builder.select_slot(HELD_SLOT).await?;
        }
    }
    let sequence = match step.act {
        Act::Click { at, face } => {
            let (x, y, z) = place.block(at);
            builder.click(x, y, z, face, centre(face)).await?
        }
        Act::Dig(at) => {
            let (x, y, z) = place.block(at);
            builder.dig(x, y, z).await?
        }
    };
    builder
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .map_err(|error| anyhow::anyhow!("{step:?} was not acknowledged: {error:#}"))
}

/// The middle of a face, where a click on it lands.
fn centre(face: u8) -> [f32; 3] {
    match face {
        UP => [0.5, 1.0, 0.5],
        NORTH => [0.5, 0.5, 0.0],
        SOUTH => [0.5, 0.5, 1.0],
        WEST => [0.0, 0.5, 0.5],
        EAST => [1.0, 0.5, 0.5],
        _ => [0.5, 0.0, 0.5],
    }
}

/// The blocks a step of the setup changes, as the builder has them before it: for a
/// click the clicked block and the one beside it, for a dig the block dug.
fn watched(builder: &Bot, step: &Step, place: Place) -> Vec<((i32, i32, i32), Option<i32>)> {
    let blocks = match step.act {
        Act::Click { at, face } => vec![place.block(at), place.block(beside(at, face))],
        Act::Dig(at) => vec![place.block(at)],
    };
    blocks
        .into_iter()
        .map(|(x, y, z)| ((x, y, z), builder.block_at(x, y, z).ok().flatten()))
        .collect()
}

fn beside(at: Rel, face: u8) -> Rel {
    let (x, y, z) = at;
    match face {
        UP => (x, y + 1, z),
        NORTH => (x, y, z - 1),
        SOUTH => (x, y, z + 1),
        WEST => (x - 1, y, z),
        EAST => (x + 1, y, z),
        _ => (x, y - 1, z),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn redstone_matches_the_official_server() {
    compare(&cases()).await;
}

#[test]
fn the_cases_keep_apart_and_within_one_region() {
    let cases = cases();
    let places: Vec<Place> = (0..cases.len()).map(|index| Place::of(index, 0)).collect();
    for (index, place) in places.iter().enumerate() {
        let (west, east) = (place.x - BOX_ACROSS, place.x + BOX_ACROSS);
        assert!(west >= 0 && east < 64, "case {index} leaves chunks 0 to 3");
        assert!(
            place.z + BOX_ACROSS < -12 || place.x - BOX_ACROSS > 12,
            "case {index} is where players join"
        );
        for other in &places[index + 1..] {
            let apart = (place.x - other.x).abs().max((place.z - other.z).abs());
            assert!(apart > 2 * BOX_ACROSS, "case {index}'s box holds another's");
        }
    }
    for case in &cases {
        for stage in &case.stages {
            if let Until::Shown(_, text) = stage.until {
                state(text);
            }
        }
    }
}
