//! A comparison with the official server of what grows and melts by light
//! (`docs/groundwork/n8-plan.md`, sections 1 and 9, comparison 3): on each server the
//! same plants, snow and ice are set out by a builder's clicks, the random ticks run for
//! some thousands of game ticks, and how many changed is held, on each server alone, to
//! the game's chance by a binomial bound at 1 in 1,000 on either side. The light at
//! each thing is placed exactly on a threshold's side: wheat at 9 (grows) and 8 (does
//! not), saplings at 9 and 8 by night, snow at 12 (melts) and 11 (stays), ice at 11 and
//! 10.
//!
//! **The cases, at midnight.** Wheat under the open sky (the crop reads the sky's light
//! without the night's darkening, so it grows); wheat in a cellar at block light 9 and
//! at 8; oak saplings under the open sky (none grow: the night darkens the sky's 15 to
//! 4) and at block light 9 and 8 at the block above them; dirt among grass (none
//! spreads by night); snow layers two steps from a torch and three from glowstone (both
//! melt) and one step farther (neither); ice in a pit, which the light reaches from
//! above, two steps from a torch and three from glowstone (melts) and one farther
//! (stays). After them, saplings given bone meal three times over: each use on a
//! sapling of stage 0 gives stage 1 at the game's 0.45, and on one of stage 1 the tree.
//! **By day**: oak saplings under the open sky (grow), and dirt among grass (spreads).
//!
//! **How the chances are reckoned.** A random tick picks three blocks of each section a
//! game tick, each of the 4,096 alike. A thing that changes at a chance `c` of its own
//! random tick has changed after `N` game ticks with `1 − (1 − c/4096)^(3N)`: a crop on
//! dry farmland alone, `1/13` (its ground's speed is 2, `(int)(25/2) + 1`); a sapling
//! `1/7`; snow and ice in enough light `1`. A dirt block among grass is turned by any of
//! the eight grass blocks around it, each of whose random ticks tries four of the 45
//! places of its 3×5×3, so `8 · (1 − (44/45)^4)`, which holds where the 3×3 is in one
//! section. What already changed when the counting begins is left out, which a chance
//! with no memory allows. Where the light keeps it from changing, nothing may change.
//!
//! **How each server is brought to its hour.** The official server: its clock is set
//! and paused (`time set`, `time pause`), random ticks are off while the builder builds
//! and the bot reads (`gamerule random_tick_speed 0`, as `Oracle::start` leaves it), and
//! the window is `tick sprint` with them at 3; its game time is asked before and after.
//! Clustine has no commands and ticks at its own pace: its world is started, stopped,
//! its clock written into the world store's table file (the region takes the later of
//! it and its own), and started again, 10,000 ticks before 13,680 for the night and
//! 1,000 for the day, since its clock cannot be paused and the builder builds in that
//! lead; the reading that begins the window waits for the hour, the window is waited
//! out in game time, which the time packets say, and has to end
//! before the hour does (a setup that takes too long fails, saying so). On both, the
//! things are read by a reader that joins for each reading: a sapling's stage is set
//! without telling clients (flags 4), so only a fresh view of the chunk shows it.
//!
//! **What differs from the plan's words**, each for Clustine's sake: the farmland is
//! dry, as the farmland item places it (Clustine has no hoe, and wetting it takes
//! random ticks of its own), and a crop on dry farmland with a crop stays; the light of
//! 9 and 8 is from crying obsidian (10) one and two steps away, which a cellar of one
//! cell holds exactly, where a torch would need a room of five; there are 64, 32 and 24
//! wheat rather than 256, which the clicks of a builder set out in a minute or two.
//!
//! **If it fails**, the message lists each case outside the bound with the count, the
//! number that could change, the game ticks, and the chance it was held to. A case of
//! the official server's outside its bound is this test's reckoning or setup at fault,
//! not Clustine; one of Clustine's alone is Clustine's. A count where nothing may
//! change is a threshold on the wrong side: the light (`region/light.rs`) or the rule
//! (`region/random_ticks/growth.rs`, `melting.rs`).
//!
//! It needs Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so it is ignored by default. Run it on a quiet machine, alone:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test growth_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine::{Config, Server};
use clustine_botswarm::{Bot, Oracle};
use clustine_data::{BlockState, items};
use clustine_format::TableFile;
use clustine_protocol::packets::play::face;

use common::{config, start_with};

const PATIENCE: Duration = Duration::from_secs(30);

/// How long a step of the setup may take to be shown.
const SETUP_PATIENCE: Duration = Duration::from_secs(5);

/// The grass of both servers' flat worlds, the dirt below it, the dirt below that, and
/// the bedrock.
const GROUND: i32 = -61;
const UNDER: i32 = GROUND - 1;
const DEEP: i32 = GROUND - 2;
const BEDROCK: i32 = GROUND - 3;

/// Where the builder waits and the readers read: above the middle of the cases, higher
/// than a tree grown among them.
const HOVER: (f64, f64, f64) = (40.5, -40.0, -40.5);

/// The builder's feet as it clicks: above the obsidian beside the saplings, and lower
/// over the cellars, whose clicks reach down to the bedrock.
const HIGH_FEET: f64 = GROUND as f64 + 3.1;
const LOW_FEET: f64 = GROUND as f64 + 1.5;

/// The game ticks the random ticks run for.
const WINDOW: u64 = 4_800;

/// The bound: a count is outside it where it is as unlikely as this on either side.
const BOUND: f64 = 0.001;

/// What bone meal does a time in a hundred, the game's `isBonemealSuccess` of a sapling.
const BONE_MEAL: f64 = 0.45;

/// The random ticks a section has a game tick, and its blocks.
const PICKS: f64 = 3.0;
const SECTION: f64 = 4_096.0;

/// The day times Clustine's worlds are started at.
const CLUSTINE_NIGHT: u64 = 13_680;
const CLUSTINE_DAY: u64 = 1_000;
/// How long before its hour Clustine's clock is started, in game ticks, for the builder
/// to build in: Clustine's clock cannot be paused, and a night's sixteen cases took
/// some 8 000 ticks to build.
const CLUSTINE_LEAD: u64 = 10_000;
/// The day times the official server is paused at.
const OFFICIAL_NIGHT: u64 = 18_000;
const OFFICIAL_DAY: u64 = 6_000;
/// Where night and day end, as the night's darkening of the sky goes.
const NIGHT_ENDS: u64 = 22_330;
const DAY_ENDS: u64 = 11_867;

type Pos = (i32, i32, i32);

/// What is watched, as it is seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Crop,
    Sapling,
    Grass,
    Snow,
    Ice,
}

/// A watched block as a reader sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Age(u8),
    Stage(u8),
    /// A sapling's place taken by a log: its tree.
    Tree,
    Dirt,
    GrassBlock,
    Snow,
    Air,
    Ice,
    Water,
    /// Any other state, by its id.
    Other(i32),
    Missing,
}

fn name_of(id: i32) -> Option<&'static str> {
    u16::try_from(id)
        .ok()
        .map(BlockState)
        .and_then(|state| state.block())
        .map(|block| block.name)
}

fn seen(bot: &Bot, (x, y, z): Pos) -> Seen {
    let Ok(Some(id)) = bot.block_at(x, y, z) else {
        return Seen::Missing;
    };
    let Some(state) = u16::try_from(id).ok().map(BlockState) else {
        return Seen::Other(id);
    };
    let Some(block) = state.block() else {
        return Seen::Other(id);
    };
    let number = |name: &str| {
        state
            .properties()
            .find(|(property, _)| *property == name)
            .and_then(|(_, value)| value.parse::<u8>().ok())
    };
    match block.name {
        "minecraft:wheat" => number("age").map_or(Seen::Other(id), Seen::Age),
        "minecraft:oak_sapling" => number("stage").map_or(Seen::Other(id), Seen::Stage),
        "minecraft:oak_log" => Seen::Tree,
        "minecraft:dirt" => Seen::Dirt,
        "minecraft:grass_block" => Seen::GrassBlock,
        "minecraft:snow" if number("layers") == Some(1) => Seen::Snow,
        "minecraft:air" => Seen::Air,
        "minecraft:ice" => Seen::Ice,
        "minecraft:water" => Seen::Water,
        _ => Seen::Other(id),
    }
}

impl Kind {
    /// Whether a thing just built is as it should be, or already moved on by its rule.
    /// Dirt among grass is to be dirt, since the grass it was is what a click that did
    /// nothing leaves; snow likewise.
    fn built(self, seen: Seen) -> bool {
        matches!(
            (self, seen),
            (Self::Crop, Seen::Age(_))
                | (Self::Sapling, Seen::Stage(_) | Seen::Tree)
                | (Self::Grass, Seen::Dirt)
                | (Self::Snow, Seen::Snow)
                | (Self::Ice, Seen::Ice | Seen::Water)
        )
    }

    /// Whether a thing seen so when the counting begins can change, and so counts.
    fn counts(self, before: Seen) -> bool {
        match (self, before) {
            (Self::Crop, Seen::Age(age)) => age < 7,
            (Self::Sapling, Seen::Stage(stage)) => stage == 0,
            (Self::Grass, Seen::Dirt) | (Self::Snow, Seen::Snow) | (Self::Ice, Seen::Ice) => true,
            _ => false,
        }
    }

    /// Whether it changed from `before` to `after`; none where `after` is nothing its
    /// rule makes of `before`.
    fn changed(self, before: Seen, after: Seen) -> Option<bool> {
        match (self, before, after) {
            (Self::Crop, Seen::Age(from), Seen::Age(to)) if to >= from => Some(to > from),
            (Self::Sapling, Seen::Stage(0), Seen::Stage(stage)) => Some(stage == 1),
            (Self::Sapling, _, Seen::Tree)
            | (Self::Grass, _, Seen::GrassBlock)
            | (Self::Snow, _, Seen::Air)
            | (Self::Ice, _, Seen::Water) => Some(true),
            (Self::Grass, _, Seen::Dirt)
            | (Self::Snow, _, Seen::Snow)
            | (Self::Ice, _, Seen::Ice) => Some(false),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Act {
    Dig(Pos),
    /// A click on the top of a block.
    Top(Pos),
}

impl Act {
    fn target(self) -> Pos {
        match self {
            Self::Dig(at) | Self::Top(at) => at,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Step {
    held: Option<i32>,
    act: Act,
}

fn dig(at: Pos) -> Step {
    Step {
        held: None,
        act: Act::Dig(at),
    }
}

/// A click with `item` on the top of the block `on`.
fn put(item: i32, on: Pos) -> Step {
    Step {
        held: Some(item),
        act: Act::Top(on),
    }
}

/// One watched thing: where it is, how it is built, and what else it needs to be so.
#[derive(Debug, Clone)]
struct Unit {
    at: Pos,
    build: Vec<Step>,
    also: Vec<(Pos, &'static str)>,
}

/// A light that a case relies on, for the check of the layout.
#[derive(Debug, Clone, Copy)]
struct Emitter {
    at: Pos,
    level: i32,
}

struct Case {
    name: &'static str,
    kind: Kind,
    /// The chance of a change at a random tick of the thing, zero where the light keeps
    /// it from changing.
    chance: f64,
    /// What is built before the units: their light.
    setup: Vec<(Step, Pos, &'static str)>,
    units: Vec<Unit>,
    /// The light each unit's rule reads, where the layout's check can work it out: the
    /// place read and the level it should be.
    light: Vec<(Pos, i32)>,
    emitters: Vec<Emitter>,
}

impl Case {
    fn new(name: &'static str, kind: Kind, chance: f64) -> Self {
        Self {
            name,
            kind,
            chance,
            setup: Vec::new(),
            units: Vec::new(),
            light: Vec::new(),
            emitters: Vec::new(),
        }
    }
}

/// A crop's chance a random tick on dry farmland with no farmland or crop of its kind
/// around: its ground's speed is 1 + 1, and `(int)(25 / 2) + 1` is 13.
const CROP: f64 = 1.0 / 13.0;
const SAPLING: f64 = 1.0 / 7.0;

/// A dirt block's chance a random tick of the section's that lands on any of the eight
/// grass blocks around it: each tries four of the 45 places of its 3×5×3.
fn grass_chance() -> f64 {
    8.0 * (1.0 - (44.0_f64 / 45.0).powi(4))
}

fn steps(from: i32, to: i32, by: usize) -> impl Iterator<Item = i32> + Clone {
    (from..=to).step_by(by)
}

fn wheat_in_the_open() -> Case {
    let mut case = Case::new("wheat under the open sky", Kind::Crop, CROP);
    for x in steps(18, 32, 2) {
        for z in steps(-78, -64, 2) {
            case.units.push(Unit {
                at: (x, GROUND + 1, z),
                build: vec![
                    dig((x, GROUND, z)),
                    put(items::FARMLAND, (x, UNDER, z)),
                    put(items::WHEAT_SEEDS, (x, GROUND, z)),
                ],
                also: vec![((x, GROUND, z), "minecraft:farmland")],
            });
        }
    }
    case
}

/// Wheat in a cellar of one cell dug below the grass, its light from crying obsidian
/// `away` steps off along x through dug cells: 10 less the steps.
fn wheat_in_a_cellar(
    name: &'static str,
    away: i32,
    chance: f64,
    xs: &[i32],
    zs: (i32, i32),
) -> Case {
    let mut case = Case::new(name, Kind::Crop, chance);
    for &x in xs {
        for z in steps(zs.0, zs.1, 2) {
            let mut build: Vec<Step> = (0..=away).map(|dx| dig((x + dx, UNDER, z))).collect();
            build.extend([
                put(items::CRYING_OBSIDIAN, (x + away, DEEP, z)),
                dig((x, DEEP, z)),
                put(items::FARMLAND, (x, BEDROCK, z)),
                put(items::WHEAT_SEEDS, (x, DEEP, z)),
            ]);
            case.units.push(Unit {
                at: (x, UNDER, z),
                build,
                also: vec![
                    ((x + away, UNDER, z), "minecraft:crying_obsidian"),
                    ((x, DEEP, z), "minecraft:farmland"),
                ],
            });
        }
    }
    case
}

/// Oak saplings on the grass, with crying obsidian on a stone `away` steps off along x
/// at the height of the block above the sapling, whose light the sapling reads, if
/// `away` is given.
fn saplings(name: &'static str, chance: f64, away: Option<i32>, xs: &[i32], zs: &[i32]) -> Case {
    let mut case = Case::new(name, Kind::Sapling, chance);
    for &x in xs {
        for &z in zs {
            let mut build = vec![put(items::OAK_SAPLING, (x, GROUND, z))];
            let mut also = Vec::new();
            if let Some(away) = away {
                build.push(put(items::STONE, (x + away, GROUND, z)));
                build.push(put(items::CRYING_OBSIDIAN, (x + away, GROUND + 1, z)));
                also.push(((x + away, GROUND + 2, z), "minecraft:crying_obsidian"));
                case.emitters.push(Emitter {
                    at: (x + away, GROUND + 2, z),
                    level: 10,
                });
                case.light.push(((x, GROUND + 2, z), 10 - away));
            }
            case.units.push(Unit {
                at: (x, GROUND + 1, z),
                build,
                also,
            });
        }
    }
    case
}

/// Dirt among grass: the grass dug and dirt put in its place. Each 3×3 is inside one
/// chunk, so that its eight grass blocks are of one section.
fn dirt_among_grass(name: &'static str, chance: f64, xs: &[i32], zs: &[i32]) -> Case {
    let mut case = Case::new(name, Kind::Grass, chance);
    for &x in xs {
        for &z in zs {
            case.units.push(Unit {
                at: (x, GROUND, z),
                build: vec![dig((x, GROUND, z)), put(items::DIRT, (x, UNDER, z))],
                also: Vec::new(),
            });
        }
    }
    case
}

/// The places on the grass `distance` steps from `(x, z)`.
fn ring((x, z): (i32, i32), distance: i32) -> Vec<(i32, i32)> {
    let mut places = Vec::new();
    for dx in -distance..=distance {
        let rest = distance - dx.abs();
        places.push((x + dx, z + rest));
        if rest != 0 {
            places.push((x + dx, z - rest));
        }
    }
    places
}

/// The snow and the ice about an emitter: snow on the grass and ice in a pit dug where
/// the grass was, taking turns round each ring.
struct Melting {
    snow_melts: Case,
    snow_stays: Case,
    ice_melts: Case,
    ice_stays: Case,
}

fn melting(torch: bool, centres: &[i32]) -> Melting {
    let (item, name, level) = if torch {
        (items::TORCH, "minecraft:torch", 14)
    } else {
        (items::GLOWSTONE, "minecraft:glowstone", 15)
    };
    // A snow layer melts in block light above 11, ice above 11 less its dampening of 1,
    // and the ice's light is a step less than the light above its pit.
    let melts = level - 12;
    let (mut snow_melts, mut snow_stays, mut ice_melts, mut ice_stays) = if torch {
        (
            Case::new("snow two steps from a torch", Kind::Snow, 1.0),
            Case::new("snow three steps from a torch", Kind::Snow, 0.0),
            Case::new("ice two steps from a torch", Kind::Ice, 1.0),
            Case::new("ice three steps from a torch", Kind::Ice, 0.0),
        )
    } else {
        (
            Case::new("snow three steps from glowstone", Kind::Snow, 1.0),
            Case::new("snow four steps from glowstone", Kind::Snow, 0.0),
            Case::new("ice three steps from glowstone", Kind::Ice, 1.0),
            Case::new("ice four steps from glowstone", Kind::Ice, 0.0),
        )
    };
    for &centre_z in centres {
        let centre = (56, centre_z);
        let emitter = Emitter {
            at: (centre.0, GROUND + 1, centre.1),
            level,
        };
        for (distance, snow, ice) in [
            (melts, &mut snow_melts, &mut ice_melts),
            (melts + 1, &mut snow_stays, &mut ice_stays),
        ] {
            for (index, (x, z)) in ring(centre, distance).into_iter().enumerate() {
                if index % 2 == 0 {
                    snow.units.push(Unit {
                        at: (x, GROUND + 1, z),
                        build: vec![put(items::SNOW, (x, GROUND, z))],
                        also: Vec::new(),
                    });
                    snow.light.push(((x, GROUND + 1, z), level - distance));
                } else {
                    ice.units.push(Unit {
                        at: (x, GROUND, z),
                        build: vec![dig((x, GROUND, z)), put(items::ICE, (x, UNDER, z))],
                        also: Vec::new(),
                    });
                    ice.light.push(((x, GROUND + 1, z), level - distance));
                }
            }
            snow.emitters.push(emitter);
            ice.emitters.push(emitter);
        }
        // The emitter is built once, before the first of its cases' units.
        snow_melts
            .setup
            .push((put(item, (centre.0, GROUND, centre.1)), emitter.at, name));
    }
    Melting {
        snow_melts,
        snow_stays,
        ice_melts,
        ice_stays,
    }
}

/// The cases of the night, the saplings for bone meal last.
fn night() -> (Vec<Case>, Case) {
    let torches = melting(true, &[-74, -63]);
    let glowstone = melting(false, &[-52, -41]);
    let cases = vec![
        wheat_in_the_open(),
        wheat_in_a_cellar(
            "wheat in a cellar at light 9",
            1,
            CROP,
            &[36, 39, 42, 45],
            (-78, -64),
        ),
        wheat_in_a_cellar(
            "wheat in a cellar at light 8",
            2,
            0.0,
            &[36, 40, 44],
            (-61, -47),
        ),
        saplings(
            "saplings under the open sky at midnight",
            0.0,
            None,
            &[18, 21, 24, 27, 30, 33],
            &[-60, -57],
        ),
        saplings(
            "saplings at night at light 9",
            SAPLING,
            Some(1),
            &[36, 40, 44],
            &[-43, -39, -35, -31],
        ),
        saplings(
            "saplings at night at light 8",
            0.0,
            Some(2),
            &[36, 41, 46],
            &[-27, -23, -19],
        ),
        dirt_among_grass(
            "dirt among grass at night",
            0.0,
            &[18, 21, 24, 27, 30],
            &[-53, -50, -45, -42],
        ),
        torches.snow_melts,
        torches.snow_stays,
        torches.ice_melts,
        torches.ice_stays,
        glowstone.snow_melts,
        glowstone.snow_stays,
        glowstone.ice_melts,
        glowstone.ice_stays,
    ];
    let mut bone_meal = saplings("saplings given bone meal", 0.0, None, &[37, 43], &[-13, -7]);
    bone_meal
        .units
        .extend(saplings("", 0.0, None, &[52, 58], &[-32, -26, -20, -14, -8]).units);
    (cases, bone_meal)
}

fn day() -> Vec<Case> {
    vec![
        saplings(
            "saplings under the open sky by day",
            SAPLING,
            None,
            &[18, 21, 24, 27, 30, 33],
            &[-36, -33, -30],
        ),
        dirt_among_grass(
            "dirt among grass by day",
            grass_chance(),
            &[18, 21, 24, 27, 30],
            &[-26, -23, -14, -11],
        ),
    ]
}

/// The chance that a thing of chance `chance` a random tick has changed after `ticks`
/// game ticks.
fn changed_within(chance: f64, ticks: u64) -> f64 {
    if chance == 0.0 {
        return 0.0;
    }
    1.0 - (1.0 - chance / SECTION).powf(PICKS * ticks as f64)
}

fn ln_choose(n: u32, k: u32) -> f64 {
    let ln_factorial = |m: u32| (2..=m).map(|i| f64::from(i).ln()).sum::<f64>();
    ln_factorial(n) - ln_factorial(k) - ln_factorial(n - k)
}

/// `P(X ≤ x)` and `P(X ≥ x)` of a binomial of `n` and `p`.
fn tails(n: u32, p: f64, x: u32) -> (f64, f64) {
    let probability = |k: u32| {
        if p <= 0.0 {
            return if k == 0 { 1.0 } else { 0.0 };
        }
        if p >= 1.0 {
            return if k == n { 1.0 } else { 0.0 };
        }
        (ln_choose(n, k) + f64::from(k) * p.ln() + f64::from(n - k) * (1.0 - p).ln()).exp()
    };
    let below = (0..=x).map(probability).sum::<f64>();
    let above = (x..=n).map(probability).sum::<f64>();
    (below.min(1.0), above.min(1.0))
}

/// Whether `x` of `n` is within the bound of a binomial of `p`, and the line saying so.
fn judged(n: u32, p: f64, x: u32) -> (bool, String) {
    let (below, above) = tails(n, p, x);
    // A case where nothing could change says nothing, which is its setup at fault.
    let within = if n == 0 {
        false
    } else if p == 0.0 {
        x == 0
    } else {
        below >= BOUND / 2.0 && above >= BOUND / 2.0
    };
    let expected = f64::from(n) * p;
    let spread = (expected * (1.0 - p)).sqrt();
    (
        within,
        format!(
            "{x} of {n}; the game's chance {p:.4} gives {expected:.1} ± {spread:.1}, \
             P(≤ {x}) {below:.2e}, P(≥ {x}) {above:.2e}"
        ),
    )
}

/// A server being compared, and how it is brought to an hour and run.
enum Side {
    Official(Oracle),
    Clustine {
        server: Server,
        world: tempfile::TempDir,
    },
}

impl Side {
    fn name(&self) -> &'static str {
        match self {
            Self::Official(_) => "the official server",
            Self::Clustine { .. } => "Clustine",
        }
    }
}

/// Starts Clustine on a new world in `directory` with its clock at `game_time` and `day`.
async fn start_clustine_at(directory: &Path, game_time: u64, day: u64) -> Result<Server> {
    let on_disk = || Config {
        world: Some(directory.to_owned()),
        ..config()
    };
    let (server, _) = start_with(on_disk()).await;
    server.stop().await;
    set_clock(directory, game_time, day)?;
    Ok(start_with(on_disk()).await.0)
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

/// The id of the overworld's clock in the time packets the bot is sent.
fn overworld_clock(bot: &Bot) -> Result<i32> {
    let clocks = bot.info.registry("minecraft:world_clock")?;
    let index = clocks
        .iter()
        .position(|name| name == "minecraft:overworld")
        .context("no overworld clock")?;
    Ok(i32::try_from(index)?)
}

/// The day less the game time, as the latest time packet with the overworld's clock
/// says.
fn day_offset(bot: &Bot) -> Result<i64> {
    let overworld = overworld_clock(bot)?;
    bot.times
        .iter()
        .rev()
        .find_map(|time| {
            time.clocks
                .iter()
                .find(|clock| clock.clock == overworld)
                .map(|clock| clock.total_ticks - time.game_time)
        })
        .context("no time packet with the overworld's clock")
}

fn latest_game_time(bot: &Bot) -> Option<i64> {
    bot.times.last().map(|time| time.game_time)
}

/// Joins as `name`, flies to the hover, and waits for the chunks of `at`.
async fn join(address: &str, name: &str, at: &[Pos]) -> Result<Bot> {
    let mut bot = Bot::join(address, name).await?;
    bot.wait_until(PATIENCE, Bot::is_loaded).await?;
    bot.set_flying(true).await?;
    let from = bot.location;
    bot.fly_to((from.0, HOVER.1, from.2), (0.0, 90.0), 1.0)
        .await?;
    bot.fly_to(HOVER, (0.0, 90.0), 1.0).await?;
    bot.wait_until(PATIENCE, |bot| {
        at.iter()
            .all(|&(x, y, z)| matches!(bot.block_at(x, y, z), Ok(Some(_))))
    })
    .await
    .context("the cases' chunks")?;
    bot.wait_until(PATIENCE, |bot| !bot.times.is_empty())
        .await
        .context("a time packet")?;
    Ok(bot)
}

/// What a reader that joins afresh sees of `at`, and the game time it saw it at.
async fn read(address: &str, name: &str, at: &[Pos]) -> Result<(Vec<Seen>, i64)> {
    let mut bot = join(address, name, at).await?;
    let seen = at.iter().map(|&place| seen(&bot, place)).collect();
    let time = latest_game_time(&bot).context("no time packet")?;
    bot.close().await?;
    Ok((seen, time))
}

/// Flies the builder to just above the column of `at`, within reach of every block of
/// `steps` (which lie within two blocks of it across): low where one reaches down to
/// the bedrock, else above the obsidian beside the saplings. Up first where it goes
/// higher, across first where it goes lower, so that it never passes through what it
/// built.
async fn go_above(bot: &mut Bot, at: Pos, steps: &[Step]) -> Result<()> {
    let deep = steps.iter().any(|step| step.act.target().1 <= DEEP);
    let feet = if deep { LOW_FEET } else { HIGH_FEET };
    let to = (f64::from(at.0) + 0.5, feet, f64::from(at.2) + 0.5);
    let from = bot.location;
    let look = (0.0, 90.0);
    if feet > from.1 {
        bot.fly_to((from.0, feet, from.2), look, 1.0).await?;
    } else {
        bot.fly_to((to.0, from.1, to.2), look, 1.0).await?;
    }
    bot.fly_to(to, look, 1.0).await
}

/// Does the steps from where the builder is, one after the other without waiting (each
/// server takes a player's packets in order), and waits for the last to be
/// acknowledged.
async fn do_steps(bot: &mut Bot, steps: &[Step]) -> Result<()> {
    let mut last = None;
    for &step in steps {
        last = Some(do_step(bot, step).await?);
    }
    if let Some(sequence) = last {
        bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
            .await
            .with_context(|| format!("{steps:?} were not acknowledged"))?;
    }
    Ok(())
}

async fn do_step(bot: &mut Bot, step: Step) -> Result<i32> {
    match step.held {
        Some(item) => {
            bot.put_in_hotbar(0, clustine_protocol::item::ItemStack::new(item, 64))
                .await?;
            bot.select_slot(0).await?;
        }
        None => {
            bot.empty_hotbar_slot(8).await?;
            bot.select_slot(8).await?;
        }
    }
    Ok(match step.act {
        Act::Dig((x, y, z)) => bot.dig(x, y, z).await?,
        Act::Top((x, y, z)) => bot.use_item_on(x, y, z, face::TOP).await?,
    })
}

fn named(bot: &Bot, (x, y, z): Pos) -> Option<&'static str> {
    bot.block_at(x, y, z).ok().flatten().and_then(name_of)
}

/// Builds every case, each unit until it is shown as built, up to three times.
async fn build(bot: &mut Bot, cases: &[&Case]) -> Result<()> {
    for case in cases {
        for (step, at, name) in &case.setup {
            go_above(bot, *at, &[*step]).await?;
            do_steps(bot, &[*step]).await?;
            bot.wait_until(SETUP_PATIENCE, |bot| named(bot, *at) == Some(name))
                .await
                .with_context(|| format!("{}: {name} at {at:?}", case.name))?;
        }
    }
    for case in cases {
        for unit in &case.units {
            let ready = |bot: &Bot| {
                case.kind.built(seen(bot, unit.at))
                    && unit
                        .also
                        .iter()
                        .all(|(at, name)| named(bot, *at) == Some(name))
            };
            let mut tries = 0;
            loop {
                go_above(bot, unit.at, &unit.build).await?;
                do_steps(bot, &unit.build).await?;
                if bot.wait_until(SETUP_PATIENCE, ready).await.is_ok() {
                    break;
                }
                tries += 1;
                ensure!(
                    tries < 3,
                    "{}: the unit at {:?} was not built: it is {:?}, and {:?}",
                    case.name,
                    unit.at,
                    seen(bot, unit.at),
                    unit.also
                        .iter()
                        .map(|(at, _)| (at, named(bot, *at)))
                        .collect::<Vec<_>>()
                );
            }
        }
    }
    let from = bot.location;
    bot.fly_to((from.0, HOVER.1, from.2), (0.0, 90.0), 1.0)
        .await?;
    bot.fly_to(HOVER, (0.0, 90.0), 1.0).await?;
    Ok(())
}

/// What one case came to on one server.
struct Counted {
    name: String,
    within: bool,
    line: String,
}

/// The counts of one stage on one server: what each case came to.
async fn run_stage(
    side: &Side,
    address: &str,
    builder: &mut Bot,
    cases: &[Case],
    bone_meal: Option<&Case>,
    (hour_begins, hour_ends): (u64, u64),
    readers: &mut u32,
) -> Result<Vec<Counted>> {
    let mut all: Vec<&Case> = cases.iter().collect();
    all.extend(bone_meal);
    eprintln!("{}: building {} cases", side.name(), all.len());
    build(builder, &all).await?;
    if let Side::Clustine { .. } = side {
        // Built in the lead before the hour: the counting begins with it.
        let offset = day_offset(builder)?;
        let in_hour = |bot: &Bot| {
            latest_game_time(bot).is_some_and(|time| {
                let day = u64::try_from((time + offset).rem_euclid(24_000)).unwrap_or(0);
                (hour_begins..hour_ends).contains(&day)
            })
        };
        builder
            .wait_until(Duration::from_secs(CLUSTINE_LEAD / 20 * 3 + 60), in_hour)
            .await
            .context("the hour's beginning")?;
    }
    eprintln!("{}: built; reading", side.name());
    let places: Vec<Pos> = cases
        .iter()
        .flat_map(|case| case.units.iter().map(|unit| unit.at))
        .collect();
    let mut reader = |readers: &mut u32| {
        *readers += 1;
        format!("Reader{readers}")
    };

    let (before, began) = read(address, &reader(readers), &places).await?;
    let ticks = match side {
        Side::Official(oracle) => {
            let game_time = |answer: String| -> Result<i64> {
                answer
                    .split("The game time is ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|number| number.parse().ok())
                    .with_context(|| format!("no game time in {answer:?}"))
            };
            let from = game_time(
                oracle
                    .command("time query gametime", &["The game time is"])
                    .await?,
            )?;
            oracle
                .command("gamerule random_tick_speed 3", &["now set to"])
                .await?;
            oracle
                .command(&format!("tick sprint {WINDOW}"), &["Sprint completed"])
                .await?;
            oracle
                .command("gamerule random_tick_speed 0", &["now set to"])
                .await?;
            let to = game_time(
                oracle
                    .command("time query gametime", &["The game time is"])
                    .await?,
            )?;
            u64::try_from(to - from)?
        }
        Side::Clustine { .. } => {
            // The hour must last the window, and the bone meal after it.
            let offset = day_offset(builder)?;
            let day = u64::try_from(began + offset)? % 24_000;
            ensure!(
                day + WINDOW + 1_200 < hour_ends,
                "the setup took Clustine until day {day}: the window would leave the hour"
            );
            let until = began + WINDOW as i64;
            builder
                .wait_until(Duration::from_secs(WINDOW / 20 * 3 + 60), |bot| {
                    latest_game_time(bot).is_some_and(|time| time >= until)
                })
                .await
                .context("the window's end")?;
            0
        }
    };
    eprintln!("{}: the window is over; reading", side.name());
    let (after, ended) = read(address, &reader(readers), &places).await?;
    let ticks = match side {
        Side::Official(_) => ticks,
        Side::Clustine { .. } => u64::try_from(ended - began)?,
    };

    let mut counted = Vec::new();
    let mut index = 0;
    for case in cases {
        let mut eligible = 0u32;
        let mut changed = 0u32;
        let mut strange = Vec::new();
        for unit in &case.units {
            let (from, to) = (before[index], after[index]);
            index += 1;
            if !case.kind.counts(from) {
                continue;
            }
            eligible += 1;
            match case.kind.changed(from, to) {
                Some(true) => changed += 1,
                Some(false) => {}
                None => strange.push(format!("{:?} from {from:?} to {to:?}", unit.at)),
            }
        }
        let p = changed_within(case.chance, ticks);
        let (within, line) = judged(eligible, p, changed);
        let within = within && strange.is_empty();
        counted.push(Counted {
            name: case.name.to_owned(),
            within,
            line: format!(
                "{line} over {ticks} game ticks{}",
                if strange.is_empty() {
                    String::new()
                } else {
                    format!("; what the rule does not make: {}", strange.join(", "))
                }
            ),
        });
    }

    if let Some(case) = bone_meal {
        eprintln!("{}: bone meal", side.name());
        counted.extend(feed_bone_meal(address, builder, case, readers, &mut reader).await?);
    }
    Ok(counted)
}

/// Bone meal on the saplings of `case`, three times over, each time read afresh: the
/// uses on a sapling of stage 0 and on one of stage 1, and how many moved it on.
async fn feed_bone_meal(
    address: &str,
    builder: &mut Bot,
    case: &Case,
    readers: &mut u32,
    reader: &mut impl FnMut(&mut u32) -> String,
) -> Result<Vec<Counted>> {
    let places: Vec<Pos> = case.units.iter().map(|unit| unit.at).collect();
    let (mut now, _) = read(address, &reader(readers), &places).await?;
    let mut stage = (0u32, 0u32);
    let mut tree = (0u32, 0u32);
    let mut strange = Vec::new();
    for _ in 0..3 {
        let mut used = Vec::new();
        for (index, unit) in case.units.iter().enumerate() {
            if let Seen::Stage(_) = now[index] {
                let feed = [put(items::BONE_MEAL, unit.at)];
                // Bone meal grows trees, which the official server does not let a
                // player fly through. So the builder feeds each sapling from three
                // blocks east of it, outside the leaves of any tree of the saplings,
                // which are six apart, and goes from one to the next high above them.
                let from = builder.location;
                let beside = (
                    f64::from(unit.at.0) + 3.5,
                    HIGH_FEET,
                    f64::from(unit.at.2) + 0.5,
                );
                let look = (0.0, 90.0);
                builder.fly_to((from.0, HOVER.1, from.2), look, 1.0).await?;
                builder
                    .fly_to((beside.0, HOVER.1, beside.2), look, 1.0)
                    .await?;
                builder.fly_to(beside, look, 1.0).await?;
                do_steps(builder, &feed).await?;
                used.push(index);
            }
        }
        let (then, _) = read(address, &reader(readers), &places).await?;
        for index in used {
            match (now[index], then[index]) {
                (Seen::Stage(0), Seen::Stage(0)) => stage.0 += 1,
                (Seen::Stage(0), Seen::Stage(1)) => stage = (stage.0 + 1, stage.1 + 1),
                (Seen::Stage(1), Seen::Stage(1)) => tree.0 += 1,
                (Seen::Stage(1), Seen::Tree) => tree = (tree.0 + 1, tree.1 + 1),
                (from, to) => strange.push(format!("{:?} from {from:?} to {to:?}", places[index])),
            }
        }
        now = then;
    }
    let from = builder.location;
    builder
        .fly_to((from.0, HOVER.1, from.2), (0.0, 90.0), 1.0)
        .await?;
    let mut counted = Vec::new();
    for (name, (trials, successes)) in [
        ("bone meal on a sapling of stage 0", stage),
        ("bone meal on a sapling of stage 1", tree),
    ] {
        let (within, line) = judged(trials, BONE_MEAL, successes);
        counted.push(Counted {
            name: name.to_owned(),
            within: within && strange.is_empty(),
            line: if strange.is_empty() {
                line
            } else {
                format!(
                    "{line}; what bone meal does not make: {}",
                    strange.join(", ")
                )
            },
        });
    }
    Ok(counted)
}

/// Both stages on one server.
async fn run_side(side: &Side, address: &str) -> Result<Vec<(String, Counted)>> {
    let mut readers = 0;
    let mut builder = Bot::join(address, "Builder").await?;
    builder.wait_until(PATIENCE, Bot::is_loaded).await?;
    builder.set_flying(true).await?;
    let (cases, bone_meal) = night();
    let mut counted = Vec::new();
    if let Side::Official(oracle) = side {
        // Rain wets farmland and darkens the sky, and Clustine has none.
        oracle
            .command("gamerule advance_weather false", &["now set to", "already"])
            .await?;
        oracle
            .command("weather clear", &["Set the weather"])
            .await?;
        set_official_time(oracle, OFFICIAL_NIGHT).await?;
        oracle
            .command("time pause", &["Paused clock", "already paused"])
            .await?;
    }
    for done in run_stage(
        side,
        address,
        &mut builder,
        &cases,
        Some(&bone_meal),
        (OFFICIAL_NIGHT, NIGHT_ENDS),
        &mut readers,
    )
    .await?
    {
        counted.push(("at midnight".to_owned(), done));
    }
    if let Side::Official(oracle) = side {
        set_official_time(oracle, OFFICIAL_DAY).await?;
        for done in run_stage(
            side,
            address,
            &mut builder,
            &day(),
            None,
            (OFFICIAL_DAY, DAY_ENDS),
            &mut readers,
        )
        .await?
        {
            counted.push(("by day".to_owned(), done));
        }
    }
    Ok(counted)
}

async fn set_official_time(oracle: &Oracle, day: u64) -> Result<()> {
    oracle
        .command(
            &format!("time set {day}"),
            &["tick(s)", "Set the time", "already set"],
        )
        .await?;
    Ok(())
}

/// Clustine's night and day, each on a world of its own started at its hour.
async fn run_clustine() -> Result<Vec<(String, Counted)>> {
    let mut counted = Vec::new();
    for (hour, label) in [(CLUSTINE_NIGHT, "at midnight"), (CLUSTINE_DAY, "by day")] {
        let world = tempfile::tempdir()?;
        let started = 24_000 + hour - CLUSTINE_LEAD;
        let server = start_clustine_at(world.path(), 100_000, started).await?;
        let address = server.address().to_string();
        let side = Side::Clustine { server, world };
        let ran = async {
            let mut builder = Bot::join(&address, "Builder").await?;
            builder.wait_until(PATIENCE, Bot::is_loaded).await?;
            builder.set_flying(true).await?;
            let offset = day_offset(&builder)?;
            let joined = latest_game_time(&builder).context("no time packet")?;
            ensure!(
                offset == started as i64 - 100_000 && joined >= 100_000,
                "Clustine's clock did not take: the join said game time {joined} and a day \
                 {offset} ahead of it"
            );
            let mut readers = 0;
            if hour == CLUSTINE_NIGHT {
                let (cases, bone_meal) = night();
                run_stage(
                    &side,
                    &address,
                    &mut builder,
                    &cases,
                    Some(&bone_meal),
                    (CLUSTINE_NIGHT, NIGHT_ENDS),
                    &mut readers,
                )
                .await
            } else {
                run_stage(
                    &side,
                    &address,
                    &mut builder,
                    &day(),
                    None,
                    (CLUSTINE_DAY, DAY_ENDS),
                    &mut readers,
                )
                .await
            }
        }
        .await;
        let Side::Clustine { server, world } = side else {
            bail!("Clustine's side")
        };
        server.stop().await;
        drop(world);
        for done in ran? {
            counted.push((label.to_owned(), done));
        }
    }
    Ok(counted)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn growth_and_melting_go_at_the_rates_of_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let address = oracle.address().to_owned();
    let side = Side::Official(oracle);
    let theirs = run_side(&side, &address).await;
    let name = side.name();
    drop(side);
    let theirs = theirs.unwrap_or_else(|error| panic!("on {name}: {error:#}"));
    let ours = run_clustine()
        .await
        .unwrap_or_else(|error| panic!("on Clustine: {error:#}"));

    let mut outside = Vec::new();
    for (server, counted) in [("the official server", &theirs), ("Clustine", &ours)] {
        println!("{server}:");
        for (hour, done) in counted {
            let mark = if done.within { " " } else { "≠" };
            println!("  {mark} {hour}, {}: {}", done.name, done.line);
            if !done.within {
                outside.push(format!("{server}, {hour}, {}: {}", done.name, done.line));
            }
        }
    }
    assert!(
        outside.is_empty(),
        "{} counts are outside the game's bound at 1 in {} (a case of the official \
         server's outside it is this test's reckoning at fault):\n  {}",
        outside.len(),
        (1.0 / BOUND) as u32,
        outside.join("\n  ")
    );
}

/// Every place each stage builds at or reads, with the case it is of.
fn layout(cases: &[&Case]) -> Vec<(Pos, &'static str)> {
    let mut places = Vec::new();
    for case in cases {
        for (_, at, _) in &case.setup {
            places.push((*at, case.name));
        }
        for unit in &case.units {
            places.push((unit.at, case.name));
            for (at, _) in &unit.also {
                places.push((*at, case.name));
            }
            for step in &unit.build {
                places.push((step.act.target(), case.name));
            }
        }
    }
    places
}

#[test]
fn the_cases_keep_to_ticking_chunks_and_out_of_each_others_way() {
    let (night, bone_meal) = night();
    let day = day();
    let mut stages: Vec<Vec<&Case>> = vec![night.iter().collect(), day.iter().collect()];
    stages[0].push(&bone_meal);
    // On the official server the day's cases are built beside the night's.
    let together: Vec<&Case> = stages.concat();
    for case in &together {
        for unit in &case.units {
            let (x, _, z) = unit.at;
            // Chunks 1 to 3 along x, one region where `CLUSTINE_TEST_PINS=0,4` divides
            // the world, and -5 to -1 along z, within two of the hover's chunk.
            assert!(
                (17..=62).contains(&x) && (-80..=-2).contains(&z),
                "{}: {unit:?}",
                case.name
            );
        }
    }
    // No two units of any case share a place, and no unit is where another case builds.
    let mut units = std::collections::BTreeMap::new();
    for case in &together {
        for unit in &case.units {
            assert!(
                units.insert(unit.at, case.name).is_none(),
                "{}: {:?} twice",
                case.name,
                unit.at
            );
        }
    }
    for (place, name) in layout(&together) {
        if let Some(owner) = units.get(&place) {
            assert_eq!(
                owner, &name,
                "{name} builds at {place:?}, a unit of {owner}"
            );
        }
    }
    // The light each case relies on is what the nearest emitter of every case gives
    // through open air (which the surface's layout is), and no other is nearer.
    let emitters: Vec<Emitter> = together
        .iter()
        .flat_map(|case| case.emitters.iter().copied())
        .collect();
    for case in &together {
        for &(at, level) in &case.light {
            let brightest = emitters
                .iter()
                .map(|emitter| {
                    let (x, y, z) = emitter.at;
                    emitter.level - (x - at.0).abs() - (y - at.1).abs() - (z - at.2).abs()
                })
                .max()
                .unwrap_or(0);
            assert_eq!(brightest, level, "{}: the light at {at:?}", case.name);
        }
    }
    // The dirt among grass: each 3×3 in one chunk, and no other unit's place in it.
    for case in together.iter().filter(|case| case.kind == Kind::Grass) {
        for unit in &case.units {
            let (x, y, z) = unit.at;
            assert!((1..=14).contains(&x.rem_euclid(16)), "{unit:?}");
            assert!((1..=14).contains(&z.rem_euclid(16)), "{unit:?}");
            for (place, name) in layout(&together) {
                let near = (place.0 - x).abs() <= 1
                    && (place.2 - z).abs() <= 1
                    && (y - 1..=y + 3).contains(&place.1);
                assert!(
                    !near || place == unit.at || place == (x, UNDER, z),
                    "{name} at {place:?} is beside {unit:?}"
                );
            }
        }
    }
    // A crop alone: no farmland or crop of its kind around it.
    for case in together.iter().filter(|case| case.kind == Kind::Crop) {
        for unit in &case.units {
            for other in &case.units {
                let (dx, dz) = (
                    (unit.at.0 - other.at.0).abs(),
                    (unit.at.2 - other.at.2).abs(),
                );
                assert!(
                    unit.at == other.at || dx > 1 || dz > 1,
                    "{unit:?} beside {other:?}"
                );
            }
        }
    }
}

#[test]
fn the_bound_takes_the_mean_and_refuses_the_far_tails() {
    let (within, _) = judged(64, 0.25, 16);
    assert!(within);
    let (within, _) = judged(64, 0.25, 0);
    assert!(!within);
    let (within, _) = judged(64, 0.25, 40);
    assert!(!within);
    assert!(judged(24, 0.0, 0).0);
    assert!(!judged(24, 0.0, 1).0);
    let (below, above) = tails(10, 0.3, 3);
    let exactly = (ln_choose(10, 3) + 3.0 * 0.3_f64.ln() + 7.0 * 0.7_f64.ln()).exp();
    assert!((below + above - 1.0 - exactly).abs() < 1e-9);
    // A sapling at its chance over the window changes about two times in five.
    let p = changed_within(SAPLING, WINDOW);
    assert!((0.38..0.41).contains(&p), "{p}");
}
