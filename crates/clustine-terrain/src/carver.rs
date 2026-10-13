// Written for Clustine from the pinned jar's own classes, by their bytecode
// (`NoiseBasedChunkGenerator.generateCarvers`, `WorldCarver`, `CaveWorldCarver`,
// `CanyonWorldCarver`, `CarvingMask`, `WorldgenRandom.setLargeFeatureSeed`, the
// providers of `net.minecraft.util.valueproviders`, `UniformHeight`). SteelMC's
// steel-core/src/worldgen/carver at 885c4b3 (AGPL-3.0-or-later, Copyright (C) 2026
// Alve Jeansson and contributors; see NOTICE) was read beside them.

//! The game's carvers: where caves and ravines are cut into a chunk after its fill
//! and its surface.
//!
//! ```
//! use clustine_terrain::biome::{BiomeSource, Dimension};
//! use clustine_terrain::fill::Terrain;
//!
//! let nether = Terrain::new(13579, Dimension::Nether);
//! let biomes = BiomeSource::new(13579, Dimension::Nether);
//! // Which blocks of the chunk at the origin the caves of the chunks around it reach.
//! let mask = nether.carving_mask(0, 0, &biomes);
//! assert!(!mask.is_empty());
//! // Never the lowest block of the terrain nor its highest seven.
//! assert_eq!((mask.min_y(), mask.max_y()), (1, 120));
//! ```
//!
//! **What the step is in the pinned jar.** In 26.3 a carver no longer changes blocks.
//! `generateCarvers` goes through the seventeen by seventeen chunks around a chunk,
//! the chunk in the middle; for each such *source chunk* it takes the carvers of the
//! biome at the source chunk's lowest corner (the biome source's own answer for the
//! cell at the height 0, not what a chunk stores), and for each carver in the
//! biome's order seeds one generator of random numbers with the world's seed plus
//! the carver's place in the list and the source chunk's position. A carver whose
//! first draw is at most its probability then walks its rooms and tunnels, or its
//! ravine, from a place in the source chunk, and sets a bit of the chunk's
//! [`Mask`] for every block it reaches in the chunk: an ellipsoid at each step of a
//! tunnel. A walk draws every number whether or not it is near the chunk, and ends
//! early only where it can no longer reach it. So the mask is a function of the
//! seed and the chunk's position ([`mask`]).
//!
//! Then the mask is applied, which is the other half of the step and is with the
//! surface, because it asks the surface's rule
//! ([`crate::fill::Terrain::apply_carving_mask`]).
//!
//! **What is different from the game's older versions**, and from what the plan for
//! this step expected: there is no carver of the Nether's own (the
//! Nether's caves are the cave carver with other numbers), no cave air, no lava
//! level and no list of blocks that may be replaced. The data's registry is
//! `worldgen/carver`. And the mask is a local of the step: a chunk does not keep it
//! and no feature asks it.
//!
//! **What it was compared with**, in October 2026: `tests/java/GameCarvers.java` has
//! the game's own generator do its fill, its surface and this step, without a server,
//! and `tests/game_carvers.rs` holds every block, every mark and the mask to it: all
//! 353,239,040 blocks of 4,780 chunks of the three dimensions on three seeds are
//! equal, and every mask bit for bit. The head of that test has the rest, and what
//! it does not see. Of SteelMC's hashes of chunks at the game's `terrain` status,
//! 2,444 of the overworld's 2,500 and 2,230 of the Nether's now hold
//! (`tests/steelmc_terrain.rs`); the others wait for the structures that bend the
//! ground.
//!
//! **What it costs** (`tests/cost.rs`, one thread, release): 1.3 ms a chunk in the
//! overworld on top of fill and surface, and 0.2 ms in the Nether. In the overworld
//! 0.8 ms of that is asking the biome source for the biomes of the 289 source
//! chunks, 0.1 ms the walks of the 67 carvers that start around a chunk (three of
//! them reach it), and 0.4 ms applying the mask. Whoever makes many chunks near one
//! another has the same source chunks asked for again and again:
//! [`crate::fill::Terrain::carving_mask_with`] takes the biomes from whoever keeps
//! them, which saves nearly all of the 0.8 ms.

use clustine_noise::math::floor;
use clustine_noise::trig::{cos, sin};
use clustine_noise::{LegacyRandom, Random};
use clustine_world::Biome;
use clustine_worldgen_data::carver::{
    Canyon, Carver, Cave, FloatProvider, IntProvider, Kind, UniformHeight,
};
use clustine_worldgen_data::carvers::{CARVERS, OF_BIOME};

/// How many chunks away a source chunk may be (`generateCarvers`' eight).
pub const REACH: i32 = 8;

/// How many source chunks one chunk's carving goes through.
pub const SOURCES: usize = ((2 * REACH + 1) * (2 * REACH + 1)) as usize;

/// How far a carver walks at most, in blocks: `getRange() * 2 - 1` chunks, which is
/// seven, although source chunks are looked at eight away.
const BLOCK_RANGE: i32 = (4 * 2 - 1) * 16;

/// The game's `float` of pi, of two pi and of half pi: 3.1415927, 6.2831855 and
/// 1.5707964, which are the nearest floats, as these are.
const PI: f32 = std::f32::consts::PI;
const TWO_PI: f32 = std::f32::consts::TAU;
const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;

/// The heights a carver reads (`WorldGenerationContext`): the lowest block of the
/// terrain and how many blocks high it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Heights {
    pub min_y: i32,
    pub depth: i32,
}

/// Which blocks of a chunk the carvers reached (`CarvingMask`): a bit for each block
/// from one above the lowest of the terrain to eight below its top.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mask {
    min_y: i32,
    max_y: i32,
    height: usize,
    words: Vec<u64>,
}

/// What a carver writes to (`CarverOutput`).
pub trait Output {
    /// The block at `x` and `z` (each in `0..16`) and the height `y` is carved.
    fn carve(&mut self, x: usize, y: i32, z: usize);
    /// The lowest and the highest height that may be carved.
    fn min_y(&self) -> i32;
    fn max_y(&self) -> i32;
}

impl Mask {
    /// The empty mask of a chunk of terrain with these heights.
    #[must_use]
    pub fn new(heights: Heights) -> Self {
        let min_y = heights.min_y + 1;
        let max_y = heights.min_y + heights.depth - 1 - 7;
        let height = (max_y - min_y + 1).max(0) as usize;
        Self {
            min_y,
            max_y,
            height,
            words: vec![0; (256 * height).div_ceil(64)],
        }
    }

    /// The lowest height the mask has a bit for. (No carver sets it: an ellipsoid
    /// ends one above the lowest height it may reach.)
    #[must_use]
    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    /// The highest height the mask has a bit for.
    #[must_use]
    pub fn max_y(&self) -> i32 {
        self.max_y
    }

    /// The game's index of a block: the heights of a column together, the columns in
    /// the order of x, then z.
    #[must_use]
    pub fn index(&self, x: usize, y: i32, z: usize) -> usize {
        (y - self.min_y) as usize + (z + (x << 4)) * self.height
    }

    /// Whether the block was reached; never outside the mask's heights.
    #[must_use]
    pub fn get(&self, x: usize, y: i32, z: usize) -> bool {
        if y < self.min_y || y > self.max_y {
            return false;
        }
        let index = self.index(x, y, z);
        self.words[index / 64] >> (index % 64) & 1 == 1
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    /// How many blocks were reached.
    #[must_use]
    pub fn count(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// The first index at or after `from` whose bit is as `set` says; the number of
    /// bits where there is none.
    fn next(&self, from: usize, set: bool) -> usize {
        let bits = 256 * self.height;
        let mut index = from;
        while index < bits {
            let word = self.words[index / 64];
            let word = if set { word } else { !word };
            let rest = word >> (index % 64);
            if rest != 0 {
                return (index + rest.trailing_zeros() as usize).min(bits);
            }
            index = (index / 64 + 1) * 64;
        }
        bits
    }

    /// The runs of indices whose bits are set, each as its first index and its last,
    /// ascending. A run can go from one column into the next.
    pub fn runs(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let bits = 256 * self.height;
        let mut from = 0;
        std::iter::from_fn(move || {
            let first = self.next(from, true);
            if first >= bits {
                return None;
            }
            let last = self.next(first, false) - 1;
            from = last + 1;
            Some((first, last))
        })
    }

    /// The reached blocks as the game visits them (`CarvingMask.visit`): for each
    /// run of set bits, each column it touches with the run's lowest and highest
    /// height in that column: `(x, z, lowest, highest)`. A column with two runs comes
    /// twice, the lower run first.
    pub fn columns(&self) -> impl Iterator<Item = (usize, usize, i32, i32)> + '_ {
        let height = self.height;
        self.runs().flat_map(move |(first, last)| {
            (first / height..=last / height).map(move |column| {
                let bottom = column * height;
                let lowest = first.saturating_sub(bottom) as i32 + self.min_y;
                let highest = (last - bottom).min(height - 1) as i32 + self.min_y;
                (column >> 4 & 15, column & 15, lowest, highest)
            })
        })
    }
}

impl Output for Mask {
    fn carve(&mut self, x: usize, y: i32, z: usize) {
        let index = self.index(x, y, z);
        self.words[index / 64] |= 1 << (index % 64);
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn max_y(&self) -> i32 {
        self.max_y
    }
}

/// An output that only says whether one block was reached.
struct Watch {
    min_y: i32,
    max_y: i32,
    at: (usize, i32, usize),
    reached: bool,
}

impl Output for Watch {
    fn carve(&mut self, x: usize, y: i32, z: usize) {
        self.reached |= (x, y, z) == self.at;
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn max_y(&self) -> i32 {
        self.max_y
    }
}

/// A carver of a source chunk that reached a block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Origin {
    /// The carver's name (`minecraft:canyon`).
    pub carver: &'static str,
    /// The chunk it started in.
    pub source: (i32, i32),
}

fn sample_float(provider: FloatProvider, random: &mut LegacyRandom) -> f32 {
    match provider {
        FloatProvider::Constant(value) => value,
        FloatProvider::Uniform {
            min_inclusive,
            max_exclusive,
        } => random.next_f32() * (max_exclusive - min_inclusive) + min_inclusive,
        FloatProvider::Trapezoid { min, max, plateau } => {
            let range = max - min;
            let slope = (range - plateau) / 2.0;
            let rest = range - slope;
            let first = random.next_f32();
            let second = random.next_f32();
            min + first * rest + second * slope
        }
    }
}

fn sample_int(provider: IntProvider, random: &mut LegacyRandom) -> i32 {
    match provider {
        IntProvider::VeryBiasedToBottom {
            min_inclusive,
            max_inclusive,
        } => {
            let first = random.next_i32_bounded(max_inclusive - min_inclusive + 1);
            let second = random.next_i32_bounded(first + 1);
            min_inclusive + random.next_i32_bounded(second + 1)
        }
    }
}

fn sample_height(provider: UniformHeight, heights: Heights, random: &mut LegacyRandom) -> i32 {
    let min = provider.min_inclusive.resolve(heights.min_y, heights.depth);
    let max = provider.max_inclusive.resolve(heights.min_y, heights.depth);
    if min > max {
        // The game warns of an empty range and draws nothing.
        return min;
    }
    random.next_i32_bounded(max - min + 1) + min
}

/// A carver's first draw: whether the source chunk starts it (`isStartChunk`).
fn starts(carver: &Carver, random: &mut LegacyRandom) -> bool {
    random.next_f32() <= carver.probability()
}

/// The middle block of a chunk along one axis (`getMiddleBlockX`).
fn middle(chunk: i32) -> f64 {
    f64::from((chunk << 4) + 8)
}

/// `WorldCarver.canReach`: whether a walk that is at `x`, `z` after `step` of
/// `distance` steps can still come near the chunk.
fn can_reach(chunk: (i32, i32), x: f64, z: f64, step: i32, distance: i32, thickness: f32) -> bool {
    let from_x = x - middle(chunk.0);
    let from_z = z - middle(chunk.1);
    let remaining = f64::from(distance - step);
    let radius = f64::from(thickness + 2.0 + 16.0);
    from_x * from_x + from_z * from_z - remaining * remaining <= radius * radius
}

/// `WorldCarver.carveEllipsoid`: every block of the chunk within the ellipsoid
/// around `x`, `y`, `z` that `skip` does not leave out. `skip` is given the block's
/// place in the ellipsoid along x, y and z, each from -1 to 1, and its height.
#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid<O: Output>(
    chunk: (i32, i32),
    x: f64,
    y: f64,
    z: f64,
    horizontal_radius: f64,
    vertical_radius: f64,
    out: &mut O,
    skip: impl Fn(f64, f64, f64, i32) -> bool,
) {
    let reach = 16.0 + horizontal_radius * 2.0;
    if (x - middle(chunk.0)).abs() > reach || (z - middle(chunk.1)).abs() > reach {
        return;
    }
    let (min_x, min_z) = (chunk.0 << 4, chunk.1 << 4);
    // Java's subtraction wraps; no ellipsoid is so far that it matters.
    let first_x = (floor(x - horizontal_radius) - min_x - 1).max(0);
    let last_x = (floor(x + horizontal_radius) - min_x).min(15);
    let lowest = (floor(y - vertical_radius) - 1).max(out.min_y());
    let highest = (floor(y + vertical_radius) + 1).min(out.max_y());
    let first_z = (floor(z - horizontal_radius) - min_z - 1).max(0);
    let last_z = (floor(z + horizontal_radius) - min_z).min(15);
    for in_x in first_x..=last_x {
        let along_x = (f64::from(min_x + in_x) + 0.5 - x) / horizontal_radius;
        for in_z in first_z..=last_z {
            let along_z = (f64::from(min_z + in_z) + 0.5 - z) / horizontal_radius;
            if along_x * along_x + along_z * along_z >= 1.0 {
                continue;
            }
            // From the top down to one above the lowest.
            let mut block_y = highest;
            while block_y > lowest {
                let along_y = (f64::from(block_y) - 0.5 - y) / vertical_radius;
                if !skip(along_x, along_y, along_z, block_y) {
                    out.carve(in_x as usize, block_y, in_z as usize);
                }
                block_y -= 1;
            }
        }
    }
}

/// What a cave leaves out of an ellipsoid: what is below its floor and what is
/// outside the sphere (`CaveWorldCarver.shouldSkip`).
fn cave_skips(floor_level: f64) -> impl Fn(f64, f64, f64, i32) -> bool + Copy {
    move |along_x, along_y, along_z, _| {
        along_y <= floor_level || along_x * along_x + along_y * along_y + along_z * along_z >= 1.0
    }
}

/// `CaveWorldCarver.carve`: the caves one source chunk starts.
fn carve_caves<O: Output>(
    cave: &Cave,
    heights: Heights,
    random: &mut LegacyRandom,
    chunk: (i32, i32),
    source: (i32, i32),
    out: &mut O,
) {
    let count = sample_int(cave.count, random);
    for _ in 0..count {
        let x = f64::from((source.0 << 4) + random.next_i32_bounded(16));
        let y = f64::from(sample_height(cave.y, heights, random));
        let z = f64::from((source.1 << 4) + random.next_i32_bounded(16));
        let horizontal = f64::from(sample_float(cave.horizontal_radius_multiplier, random));
        let vertical = f64::from(sample_float(cave.vertical_radius_multiplier, random));
        let start_vertical = f64::from(sample_float(cave.start_vertical_radius_multiplier, random));
        let skip = cave_skips(f64::from(sample_float(cave.floor_level, random)));
        let mut tunnels = 1;
        if random.next_i32_bounded(4) == 0 {
            // A room, and more tunnels from it.
            let room_vertical =
                f64::from(sample_float(cave.room_vertical_radius_multiplier, random));
            let thickness = 1.0 + random.next_f32() * 6.0;
            // `Mth.sin` of the game's half pi as a double: the table's entry for it.
            let radius = 1.5 + f64::from(sin(f64::from(HALF_PI)) * thickness);
            carve_ellipsoid(
                chunk,
                x + 1.0,
                y,
                z,
                radius,
                radius * room_vertical,
                out,
                skip,
            );
            tunnels += random.next_i32_bounded(4);
        }
        for _ in 0..tunnels {
            let yaw = random.next_f32() * TWO_PI;
            let pitch = (random.next_f32() - 0.5) / 4.0;
            let mut thickness = sample_float(cave.thickness, random);
            if cave.weird_thickness_bias && random.next_i32_bounded(10) == 0 {
                let first = random.next_f32();
                let second = random.next_f32();
                thickness *= first * second * 3.0 + 1.0;
            }
            let distance = BLOCK_RANGE - random.next_i32_bounded(BLOCK_RANGE / 4);
            let seed = random.next_i64();
            tunnel(
                chunk,
                seed,
                [x, y, z],
                (horizontal, vertical),
                thickness,
                yaw,
                pitch,
                (0, distance),
                start_vertical,
                out,
                skip,
            );
        }
    }
}

/// `CaveWorldCarver.createTunnel`: a tunnel from `at`, of the steps `steps.0` to
/// `steps.1`, which splits into two at one step if it is thick.
#[allow(clippy::too_many_arguments)]
fn tunnel<O: Output>(
    chunk: (i32, i32),
    seed: i64,
    at: [f64; 3],
    multipliers: (f64, f64),
    thickness: f32,
    mut yaw: f32,
    mut pitch: f32,
    steps: (i32, i32),
    y_scale: f64,
    out: &mut O,
    skip: impl Fn(f64, f64, f64, i32) -> bool + Copy,
) {
    let mut random = LegacyRandom::from_seed(seed as u64);
    let [mut x, mut y, mut z] = at;
    let (first, distance) = steps;
    let split = random.next_i32_bounded(distance / 2) + distance / 4;
    let steep = random.next_i32_bounded(6) == 0;
    let mut yaw_change = 0.0_f32;
    let mut pitch_change = 0.0_f32;
    for step in first..distance {
        let radius =
            1.5 + f64::from(sin(f64::from(PI * step as f32 / distance as f32)) * thickness);
        let vertical_radius = radius * y_scale;
        let level = cos(f64::from(pitch));
        x += f64::from(cos(f64::from(yaw)) * level);
        y += f64::from(sin(f64::from(pitch)));
        z += f64::from(sin(f64::from(yaw)) * level);
        pitch *= if steep { 0.92 } else { 0.7 };
        pitch += pitch_change * 0.1;
        yaw += yaw_change * 0.1;
        pitch_change *= 0.9;
        yaw_change *= 0.75;
        let (one, two, three) = (random.next_f32(), random.next_f32(), random.next_f32());
        pitch_change += (one - two) * three * 2.0;
        let (one, two, three) = (random.next_f32(), random.next_f32(), random.next_f32());
        yaw_change += (one - two) * three * 4.0;
        if step == split && thickness > 1.0 {
            for turn in [-HALF_PI, HALF_PI] {
                let seed = random.next_i64();
                let thickness = random.next_f32() * 0.5 + 0.5;
                tunnel(
                    chunk,
                    seed,
                    [x, y, z],
                    multipliers,
                    thickness,
                    yaw + turn,
                    pitch / 3.0,
                    (step, distance),
                    1.0,
                    out,
                    skip,
                );
            }
            return;
        }
        if random.next_i32_bounded(4) == 0 {
            continue;
        }
        if !can_reach(chunk, x, z, step, distance, thickness) {
            return;
        }
        carve_ellipsoid(
            chunk,
            x,
            y,
            z,
            radius * multipliers.0,
            vertical_radius * multipliers.1,
            out,
            skip,
        );
    }
}

/// `CanyonWorldCarver.carve` and `doCarve`: the ravine one source chunk starts.
fn carve_canyon<O: Output>(
    canyon: &Canyon,
    heights: Heights,
    random: &mut LegacyRandom,
    chunk: (i32, i32),
    source: (i32, i32),
    out: &mut O,
) {
    let mut x = f64::from((source.0 << 4) + random.next_i32_bounded(16));
    let mut y = f64::from(sample_height(canyon.y, heights, random));
    let mut z = f64::from((source.1 << 4) + random.next_i32_bounded(16));
    let mut yaw = random.next_f32() * TWO_PI;
    let mut pitch = sample_float(canyon.vertical_rotation, random);
    let y_scale = f64::from(sample_float(canyon.y_scale, random));
    let thickness = sample_float(canyon.thickness, random);
    let distance = (BLOCK_RANGE as f32 * sample_float(canyon.distance_factor, random)) as i32;
    let mut random = LegacyRandom::from_seed(random.next_i64() as u64);

    // A width for each height of the terrain, squared (`initWidthFactors`).
    let mut widths = Vec::with_capacity(heights.depth.max(0) as usize);
    let mut width = 1.0_f32;
    for height in 0..heights.depth {
        if height == 0 || random.next_i32_bounded(canyon.width_smoothness) == 0 {
            let first = random.next_f32();
            let second = random.next_f32();
            width = 1.0 + first * second;
        }
        widths.push(width * width);
    }
    let widths = &widths;
    let min_y = heights.min_y;
    let skip = move |along_x: f64, along_y: f64, along_z: f64, block_y: i32| {
        let width = f64::from(widths[(block_y - min_y - 1) as usize]);
        (along_x * along_x + along_z * along_z) * width + along_y * along_y / 6.0 >= 1.0
    };

    let mut yaw_change = 0.0_f32;
    let mut pitch_change = 0.0_f32;
    for step in 0..distance {
        let mut radius =
            1.5 + f64::from(sin(f64::from(step as f32 * PI / distance as f32)) * thickness);
        let mut vertical_radius = radius * y_scale;
        radius *= f64::from(sample_float(canyon.horizontal_radius_factor, &mut random));
        // `updateVerticalRadius`: highest in the middle of the walk, if the data
        // says so, and a little uneven.
        let middle = 1.0 - (0.5 - step as f32 / distance as f32).abs() * 2.0;
        let factor =
            canyon.vertical_radius_default_factor + canyon.vertical_radius_center_factor * middle;
        let uneven = random.next_f32() * (1.0 - 0.75) + 0.75;
        vertical_radius = f64::from(factor) * vertical_radius * f64::from(uneven);

        let level = cos(f64::from(pitch));
        let rise = sin(f64::from(pitch));
        x += f64::from(cos(f64::from(yaw)) * level);
        y += f64::from(rise);
        z += f64::from(sin(f64::from(yaw)) * level);
        pitch *= 0.7;
        pitch += pitch_change * 0.05;
        yaw += yaw_change * 0.05;
        pitch_change *= 0.8;
        yaw_change *= 0.5;
        let (one, two, three) = (random.next_f32(), random.next_f32(), random.next_f32());
        pitch_change += (one - two) * three * 2.0;
        let (one, two, three) = (random.next_f32(), random.next_f32(), random.next_f32());
        yaw_change += (one - two) * three * 4.0;
        if random.next_i32_bounded(4) == 0 {
            continue;
        }
        if !can_reach(chunk, x, z, step, distance, thickness) {
            return;
        }
        carve_ellipsoid(chunk, x, y, z, radius, vertical_radius, out, skip);
    }
}

/// One carver of one source chunk, after it was found to start there.
fn carve<O: Output>(
    carver: &Carver,
    heights: Heights,
    random: &mut LegacyRandom,
    chunk: (i32, i32),
    source: (i32, i32),
    out: &mut O,
) {
    match &carver.kind {
        Kind::Cave(cave) => carve_caves(cave, heights, random, chunk, source, out),
        Kind::Canyon(canyon) => carve_canyon(canyon, heights, random, chunk, source, out),
    }
}

/// Goes through the source chunks of `chunk` and their carvers in the game's order,
/// and gives `each` every carver that starts: the carver, its source chunk and the
/// generator of random numbers after the first draw.
fn sources(
    seed: i64,
    chunk: (i32, i32),
    mut biome_of_source: impl FnMut(i32, i32) -> Biome,
    mut each: impl FnMut(&'static Carver, (i32, i32), &mut LegacyRandom),
) {
    let mut random = LegacyRandom::from_seed(0);
    for across_x in -REACH..=REACH {
        for across_z in -REACH..=REACH {
            // Beyond the world's edge the game's sum wraps as well.
            let source = (
                chunk.0.wrapping_add(across_x),
                chunk.1.wrapping_add(across_z),
            );
            let biome = biome_of_source(source.0, source.1);
            for (place, carver) in OF_BIOME[usize::from(biome.0)].iter().enumerate() {
                let carver = &CARVERS[*carver];
                random.set_large_feature_seed(seed.wrapping_add(place as i64), source.0, source.1);
                if starts(carver, &mut random) {
                    each(carver, source, &mut random);
                }
            }
        }
    }
}

/// The carving mask of the chunk `chunk` in a world with the seed `seed` and terrain
/// of these heights: which of its blocks the carvers of the chunks around it reach.
/// `biome_of_source` gives the biome a source chunk takes its carvers from: the
/// biome of the cell at the chunk's lowest corner and the height 0, which is the
/// cell `(4 x, 0, 4 z)` for the chunk `(x, z)`. It is asked once for each of the
/// [`SOURCES`] chunks, x counting most.
#[must_use]
pub fn mask(
    seed: i64,
    heights: Heights,
    chunk: (i32, i32),
    biome_of_source: impl FnMut(i32, i32) -> Biome,
) -> Mask {
    let mut mask = Mask::new(heights);
    sources(seed, chunk, biome_of_source, |carver, source, random| {
        carve(carver, heights, random, chunk, source, &mut mask);
    });
    mask
}

/// Which carvers of which source chunks reached the block at `x`, `z` (each in
/// `0..16`) and the height `y` of the chunk `chunk`, in the game's order: for
/// looking into a difference. As costly as [`mask`].
#[must_use]
pub fn origins(
    seed: i64,
    heights: Heights,
    chunk: (i32, i32),
    biome_of_source: impl FnMut(i32, i32) -> Biome,
    (x, y, z): (usize, i32, usize),
) -> Vec<Origin> {
    let empty = Mask::new(heights);
    let mut origins = Vec::new();
    sources(seed, chunk, biome_of_source, |carver, source, random| {
        let mut watch = Watch {
            min_y: empty.min_y,
            max_y: empty.max_y,
            at: (x, y, z),
            reached: false,
        };
        carve(carver, heights, random, chunk, source, &mut watch);
        if watch.reached {
            origins.push(Origin {
                carver: carver.name,
                source,
            });
        }
    });
    origins
}

/// Every carver that starts in the [`SOURCES`] chunks around `chunk`, in the game's
/// order, with the number of the chunk's blocks it reaches, which is nought for
/// most: for saying where the time goes and what carved a chunk. Costlier than
/// [`mask`], which it does once for each carver.
#[must_use]
pub fn started(
    seed: i64,
    heights: Heights,
    chunk: (i32, i32),
    biome_of_source: impl FnMut(i32, i32) -> Biome,
) -> Vec<(Origin, usize)> {
    let mut started = Vec::new();
    sources(seed, chunk, biome_of_source, |carver, source, random| {
        let mut one = Mask::new(heights);
        carve(carver, heights, random, chunk, source, &mut one);
        let origin = Origin {
            carver: carver.name,
            source,
        };
        started.push((origin, one.count()));
    });
    started
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERWORLD: Heights = Heights {
        min_y: -64,
        depth: 384,
    };

    fn biome(name: &str) -> Biome {
        let number = clustine_worldgen_data::biomes::CLIMATE
            .iter()
            .position(|biome| biome.name == name)
            .expect("a biome of the game");
        Biome(number as u16)
    }

    #[test]
    fn a_mask_has_a_bit_for_all_but_the_lowest_block_and_the_highest_seven() {
        let mask = Mask::new(OVERWORLD);
        assert_eq!((mask.min_y(), mask.max_y()), (-63, 312));
        assert!(mask.is_empty());
        assert_eq!(mask.runs().count(), 0);
        let nether = Mask::new(Heights {
            min_y: 0,
            depth: 128,
        });
        assert_eq!((nether.min_y(), nether.max_y()), (1, 120));
    }

    #[test]
    fn a_mask_is_visited_run_by_run_as_the_game_visits_it() {
        let mut mask = Mask::new(Heights {
            min_y: 0,
            depth: 16,
        });
        // Heights 1 to 8: eight to a column.
        assert_eq!((mask.min_y(), mask.max_y()), (1, 8));
        mask.carve(0, 2, 0);
        mask.carve(0, 3, 0);
        mask.carve(0, 6, 0);
        // The top of one column and the bottom of the next are one run.
        mask.carve(2, 7, 5);
        mask.carve(2, 8, 5);
        mask.carve(2, 1, 6);
        mask.carve(15, 8, 15);
        assert_eq!(mask.count(), 7);
        assert!(mask.get(0, 3, 0) && !mask.get(0, 4, 0) && !mask.get(0, 0, 0));
        assert!(!mask.get(0, 9, 0));
        let runs: Vec<(usize, usize)> = mask.runs().collect();
        assert_eq!(
            runs,
            [
                (1, 2),
                (5, 5),
                ((2 * 16 + 5) * 8 + 6, (2 * 16 + 6) * 8),
                (255 * 8 + 7, 255 * 8 + 7)
            ]
        );
        let columns: Vec<(usize, usize, i32, i32)> = mask.columns().collect();
        assert_eq!(
            columns,
            [
                (0, 0, 2, 3),
                (0, 0, 6, 6),
                (2, 5, 7, 8),
                (2, 6, 1, 1),
                (15, 15, 8, 8)
            ]
        );
    }

    #[test]
    fn a_trapezoid_draws_twice_and_a_constant_not_at_all() {
        let mut random = LegacyRandom::from_seed(7);
        let mut same = LegacyRandom::from_seed(7);
        assert_eq!(
            sample_float(FloatProvider::Constant(3.0), &mut random).to_bits(),
            3.0_f32.to_bits()
        );
        assert_eq!(random.state(), same.state());
        let drawn = sample_float(
            FloatProvider::Trapezoid {
                min: 0.0,
                max: 3.0,
                plateau: 1.0,
            },
            &mut random,
        );
        let (first, second) = (same.next_f32(), same.next_f32());
        assert_eq!(
            drawn.to_bits(),
            (0.0 + first * 2.0 + second * 1.0).to_bits()
        );
        assert_eq!(random.state(), same.state());
    }

    #[test]
    fn a_number_very_biased_to_the_bottom_is_a_draw_below_a_draw_below_a_draw() {
        let provider = IntProvider::VeryBiasedToBottom {
            min_inclusive: 0,
            max_inclusive: 14,
        };
        let mut random = LegacyRandom::from_seed(99);
        let mut counts = [0_usize; 15];
        for _ in 0..10_000 {
            counts[sample_int(provider, &mut random) as usize] += 1;
        }
        // Nought is the most frequent by far, and fourteen is rare.
        assert!(
            counts[0] > 3_500 && counts[0] > counts[1] + 1_500,
            "{counts:?}"
        );
        assert!(counts[1] > counts[2] && counts[2] > counts[4], "{counts:?}");
        assert!(counts[14] < 20, "{counts:?}");
    }

    #[test]
    fn a_height_between_two_anchors_includes_both_and_an_empty_range_draws_nothing() {
        let provider = UniformHeight {
            min_inclusive: clustine_worldgen_data::Anchor::AboveBottom(8),
            max_inclusive: clustine_worldgen_data::Anchor::Absolute(-50),
        };
        let mut random = LegacyRandom::from_seed(1);
        let mut seen = [false; 7];
        for _ in 0..500 {
            let height = sample_height(provider, OVERWORLD, &mut random);
            seen[(height + 56) as usize] = true;
        }
        assert_eq!(seen, [true; 7]);
        let empty = UniformHeight {
            min_inclusive: clustine_worldgen_data::Anchor::Absolute(10),
            max_inclusive: clustine_worldgen_data::Anchor::Absolute(9),
        };
        let before = random.state();
        assert_eq!(sample_height(empty, OVERWORLD, &mut random), 10);
        assert_eq!(random.state(), before);
    }

    #[test]
    fn an_ellipsoid_is_carved_only_within_the_chunk_and_the_masks_heights() {
        let mut mask = Mask::new(OVERWORLD);
        // Around the corner of the chunk at the origin, eight blocks wide.
        carve_ellipsoid((0, 0), 0.0, 20.0, 0.0, 4.0, 4.0, &mut mask, |_, _, _, _| {
            false
        });
        assert!(mask.get(0, 20, 0) && mask.get(3, 20, 0) && !mask.get(4, 20, 0));
        // The column of the centre, from one above the floor of the lowest height.
        assert!(mask.get(0, 24, 0) && mask.get(0, 25, 0) && !mask.get(0, 26, 0));
        assert!(mask.get(0, 16, 0) && !mask.get(0, 15, 0));
        // Too far from the chunk's middle: nothing.
        let mut far = Mask::new(OVERWORLD);
        carve_ellipsoid((0, 0), 40.0, 20.0, 8.0, 4.0, 4.0, &mut far, |_, _, _, _| {
            false
        });
        assert!(far.is_empty());
        // The bottom of the mask is never carved, nor anything above its top.
        let mut low = Mask::new(OVERWORLD);
        carve_ellipsoid((0, 0), 8.0, -62.0, 8.0, 3.0, 9.0, &mut low, |_, _, _, _| {
            false
        });
        assert!(low.get(8, -62, 8) && !low.get(8, -63, 8));
    }

    #[test]
    fn a_cave_leaves_a_floor() {
        let skip = cave_skips(-0.7);
        assert!(skip(0.0, -0.7, 0.0, 0) && skip(0.0, -0.9, 0.0, 0));
        assert!(!skip(0.0, -0.69, 0.0, 0));
        assert!(skip(0.8, 0.0, 0.6, 0) && !skip(0.7, 0.0, 0.6, 0));
    }

    #[test]
    fn a_mask_is_the_same_every_time_and_another_for_another_chunk_or_seed() {
        let plains = biome("minecraft:plains");
        let of = |seed: i64, chunk: (i32, i32)| mask(seed, OVERWORLD, chunk, |_, _| plains);
        let first = of(13579, (3, -7));
        assert_eq!(first, of(13579, (3, -7)));
        assert!(!first.is_empty());
        assert_ne!(first, of(13579, (4, -7)));
        assert_ne!(first, of(13580, (3, -7)));
    }

    #[test]
    fn a_biome_without_carvers_carves_nothing() {
        let end = biome("minecraft:the_end");
        let mut asked = Vec::new();
        let mask = mask(13579, OVERWORLD, (0, 0), |x, z| {
            asked.push((x, z));
            end
        });
        assert!(mask.is_empty());
        // Every source chunk once, x counting most.
        assert_eq!(asked.len(), SOURCES);
        assert_eq!(
            (asked[0], asked[1], asked[17]),
            ((-8, -8), (-8, -7), (-7, -8))
        );
    }

    #[test]
    fn every_block_of_a_mask_has_a_carver_it_came_from() {
        let plains = biome("minecraft:plains");
        let chunk = (3, -7);
        let mask = mask(13579, OVERWORLD, chunk, |_, _| plains);
        let (first, _) = mask.runs().next().expect("the mask is not empty");
        let (column, up) = (first / 376, first % 376);
        let block = (column >> 4, up as i32 - 63, column & 15);
        assert!(mask.get(block.0, block.1, block.2));
        let origins = origins(13579, OVERWORLD, chunk, |_, _| plains, block);
        assert!(!origins.is_empty());
        for origin in &origins {
            assert!((origin.source.0 - chunk.0).abs() <= REACH);
            assert!(origin.carver.starts_with("minecraft:"));
        }
        // A block that was not reached came from none.
        let not = (0..16)
            .flat_map(|x| (0..16).map(move |z| (x, 200, z)))
            .find(|(x, y, z)| !mask.get(*x, *y, *z))
            .expect("the caves do not reach every column at that height");
        assert!(origins_of_none(chunk, plains, not));
        // The carvers that reach the chunk are those the blocks came from, and
        // together they reach at least as many blocks as the mask has.
        let started = started(13579, OVERWORLD, chunk, |_, _| plains);
        let reaching: Vec<&(Origin, usize)> =
            started.iter().filter(|(_, blocks)| *blocks != 0).collect();
        assert!(!reaching.is_empty() && reaching.len() < started.len());
        assert!(reaching.iter().any(|(origin, _)| *origin == origins[0]));
        assert!(reaching.iter().map(|(_, blocks)| blocks).sum::<usize>() >= mask.count());
    }

    fn origins_of_none(chunk: (i32, i32), biome: Biome, block: (usize, i32, usize)) -> bool {
        origins(13579, OVERWORLD, chunk, |_, _| biome, block).is_empty()
    }
}
