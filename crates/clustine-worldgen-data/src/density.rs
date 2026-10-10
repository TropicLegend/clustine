// Adapted from SteelMC, steel-worldgen/build/density/transpiler/codegen_expr.rs (the
// formulas its scalar emitter writes inline), steel-worldgen/src/density/spline_eval.rs,
// steel-worldgen/src/density/traits.rs and steel-worldgen/src/noise/end_islands.rs at
// 885c4b3 (AGPL-3.0-or-later, Copyright (C) 2026 Alve Jeansson and contributors; see
// NOTICE). Changed for Clustine in October 2026: scalar, stable Rust; the formulas are
// functions that the emitted code calls instead of text repeated in it; splines are
// read from static data; `interpolated` is the game's value at a single position;
// minimum, maximum and clamp tell the two zeros apart as the game's do.

//! What the emitted noise routers are written against: the trait they implement, the
//! types of their static data, and the arithmetic of the game's density functions.
//!
//! A density function gives a `float` at a block position. Since 26.3 every step of
//! one is computed in single precision; only the coordinates handed to a noise are
//! doubles. Each function here is one step, in the game's order of operations, so
//! that the emitted code is a list of calls and the bits are decided in one place.
//! They were settled against the game itself: the values of the pinned jar's own
//! classes at the positions under `reference/` are what `tests/router_reference.rs`
//! compares with.

use clustine_noise::spline::{find_interval, hermite_interpolate};
use clustine_noise::{
    BlendedNoise, LegacyRandom, NameHash, NormalNoise, PositionalRandom, Random, RandomSplitter,
    SimplexNoise, Xoroshiro,
};

/// The entries of a noise router, in the order of the game's record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RouterEntry {
    Temperature,
    Vegetation,
    Continents,
    Erosion,
    Depth,
    Ridges,
    /// About where the surface is, for a whole chunk at a time.
    ChunkSurfaceLevel,
    /// Positive where there is ground.
    FinalDensity,
}

impl RouterEntry {
    pub const ALL: [Self; 8] = [
        Self::Temperature,
        Self::Vegetation,
        Self::Continents,
        Self::Erosion,
        Self::Depth,
        Self::Ridges,
        Self::ChunkSurfaceLevel,
        Self::FinalDensity,
    ];

    /// The entry's name in the game's data.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Temperature => "temperature",
            Self::Vegetation => "vegetation",
            Self::Continents => "continents",
            Self::Erosion => "erosion",
            Self::Depth => "depth",
            Self::Ridges => "ridges",
            Self::ChunkSurfaceLevel => "chunk_surface_level",
            Self::FinalDensity => "final_density",
        }
    }
}

/// The functions of a dimension's aquifers, in the order of the game's record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AquiferEntry {
    Barrier,
    FluidLevelFloodedness,
    FluidLevelSpread,
    Lava,
    /// Positive where no aquifer may be.
    Exclusion,
    SurfaceLevel,
}

impl AquiferEntry {
    pub const ALL: [Self; 6] = [
        Self::Barrier,
        Self::FluidLevelFloodedness,
        Self::FluidLevelSpread,
        Self::Lava,
        Self::Exclusion,
        Self::SurfaceLevel,
    ];

    /// The entry's name in the game's data.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Barrier => "barrier",
            Self::FluidLevelFloodedness => "fluid_level_floodedness",
            Self::FluidLevelSpread => "fluid_level_spread",
            Self::Lava => "lava",
            Self::Exclusion => "exclusion",
            Self::SurfaceLevel => "surface_level",
        }
    }
}

/// The noise router of one dimension for one seed: every density function that the
/// dimension's noise settings name, at any block position.
///
/// A value is what the game's `RandomState.sampleBlockValueUncached` gives for the
/// same function, seed and position, bit for bit, in a world without structures (the
/// beardifier adds nothing). That is the value at a single position: a function the
/// data marks `interpolated` is the interpolation between the corners of its cell.
pub trait NoiseRouter: Send + Sync + Sized {
    /// What a caller keeps between calls so that a function the data marks `cache` is
    /// not computed again for the position it was last asked at. It never changes a
    /// value. One per thread; a fresh one is `Default`.
    type Memory: Default + Clone + Send;

    /// The router of the world with this seed.
    fn new(seed: i64) -> Self;

    /// The rest of the dimension's noise settings.
    fn settings(&self) -> &'static NoiseSettings;

    /// One entry of the router at a block position.
    fn sample(&self, memory: &mut Self::Memory, entry: RouterEntry, x: i32, y: i32, z: i32) -> f32;

    /// One function of the aquifers at a block position. A dimension without aquifers
    /// ([`NoiseSettings::has_aquifers`]) answers zero.
    fn aquifer(
        &self,
        memory: &mut Self::Memory,
        entry: AquiferEntry,
        x: i32,
        y: i32,
        z: i32,
    ) -> f32;

    /// A density function of the game's registry by its name
    /// (`minecraft:overworld/continents`), or `None` for one that the dimension's
    /// settings do not reach. The spawn target names its functions so.
    fn named(&self, memory: &mut Self::Memory, name: &str, x: i32, y: i32, z: i32) -> Option<f32>;
}

/// The parameters of a noise, from `worldgen/noise`.
#[derive(Debug)]
pub struct NoiseParameters {
    /// The noise's name (`minecraft:erosion`), which seeds it.
    pub name: &'static str,
    pub base_octave: i32,
    pub base_amplitude: f64,
    pub octave_count: i32,
    pub normalise: bool,
    /// One for each octave, or none where every octave counts in full.
    pub amplitude_modifiers: &'static [f64],
}

/// What a dimension's noise settings hold beside the functions.
#[derive(Debug)]
pub struct NoiseSettings {
    /// The settings' name (`minecraft:overworld`).
    pub name: &'static str,
    /// The lowest block of the terrain and how many blocks high it is.
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    /// Whether the dimension's random numbers are `java.util.Random`'s.
    pub legacy_random_source: bool,
    pub disable_mob_generation: bool,
    pub has_aquifers: bool,
    /// The state ids of what the ground and the seas are made of before the material
    /// rules.
    pub default_block: u32,
    pub default_fluid: u32,
    /// The name of the dimension's material rule.
    pub material_rule: &'static str,
    /// A place to start in satisfies one of these: every range of it holds.
    pub spawn_target: &'static [&'static [SpawnRange]],
}

/// A density function, by its name, with the lowest and highest value it may have.
#[derive(Debug)]
pub struct SpawnRange {
    pub function: &'static str,
    pub min: f32,
    pub max: f32,
}

/// A cubic spline as static data. The emitted router has a method that evaluates the
/// coordinate with a given number.
#[derive(Debug)]
pub struct Spline {
    pub coordinate: u16,
    /// Ascending, and as many as there are derivatives and values.
    pub locations: &'static [f32],
    pub derivatives: &'static [f32],
    pub values: &'static [SplineValue],
}

#[derive(Debug)]
pub enum SplineValue {
    Constant(f32),
    Spline(&'static Spline),
}

/// A spline's value. `coordinate` evaluates the coordinate with a given number; it is
/// asked once for this spline, and of the nested splines only the two that the
/// coordinate lies between are evaluated.
pub fn spline(spline: &Spline, coordinate: &mut impl FnMut(u16) -> f32) -> f32 {
    let input = coordinate(spline.coordinate);
    let locations = spline.locations;
    let derivatives = spline.derivatives;
    let Some(last) = locations.len().checked_sub(1) else {
        return 0.0;
    };
    let mut value_at = |index: usize| match &spline.values[index] {
        SplineValue::Constant(value) => *value,
        SplineValue::Spline(nested) => self::spline(nested, coordinate),
    };

    let start = find_interval(locations, input);
    if start < 0 {
        return value_at(0) + derivatives[0] * (input - locations[0]);
    }
    let start = start as usize;
    if start == last {
        return value_at(last) + derivatives[last] * (input - locations[last]);
    }
    let low = value_at(start);
    let high = value_at(start + 1);
    hermite_interpolate(
        locations[start],
        locations[start + 1],
        low,
        high,
        derivatives[start],
        derivatives[start + 1],
        input,
    )
}

/// The last value of each function the data marks `cache`, with the position it was
/// computed at.
#[derive(Clone, Debug)]
pub struct Memory<const N: usize> {
    slots: [Slot; N],
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    filled: bool,
    position: [i32; 3],
    value: f32,
}

impl<const N: usize> Default for Memory<N> {
    fn default() -> Self {
        Self {
            slots: [Slot {
                filled: false,
                position: [0; 3],
                value: 0.0,
            }; N],
        }
    }
}

impl<const N: usize> Memory<N> {
    /// The value remembered in `slot`, if it is the one for this position.
    #[inline]
    #[must_use]
    pub fn recall(&self, slot: usize, x: i32, y: i32, z: i32) -> Option<f32> {
        let slot = &self.slots[slot];
        (slot.filled && slot.position == [x, y, z]).then_some(slot.value)
    }

    #[inline]
    pub fn remember(&mut self, slot: usize, x: i32, y: i32, z: i32, value: f32) {
        self.slots[slot] = Slot {
            filled: true,
            position: [x, y, z],
            value,
        };
    }
}

/// What the beardifier adds where no structure is near: nothing. The game binds the
/// beardifier to the chunk that is being filled; a router by itself has none.
pub const NO_BEARD: f32 = 0.0;

/// The factory every noise of a dimension is seeded from, by its name.
#[must_use]
pub fn splitter(seed: i64, legacy_random_source: bool) -> RandomSplitter {
    if legacy_random_source {
        LegacyRandom::from_seed(seed as u64).next_positional()
    } else {
        Xoroshiro::from_seed(seed as u64).next_positional()
    }
}

/// A noise of the dimension whose factory is `splitter`.
#[must_use]
pub fn noise(splitter: &RandomSplitter, parameters: &NoiseParameters) -> NormalNoise {
    NormalNoise::create_with_params(
        splitter,
        parameters.name,
        parameters.base_octave,
        parameters.base_amplitude,
        parameters.octave_count,
        parameters.normalise,
        parameters.amplitude_modifiers,
    )
}

/// The two noises of the Nether's biomes in a dimension with the old random numbers:
/// `minecraft:nether/temperature` (`which` 0) and `minecraft:nether/vegetation`
/// (`which` 1). The game does not seed them by their names: each is made as before
/// 1.18, octave after octave from `java.util.Random` of the world's seed plus
/// `which`, with two octaves from -7.
#[must_use]
pub fn legacy_nether_noise(seed: i64, which: i64) -> NormalNoise {
    let mut random = LegacyRandom::from_seed(seed.wrapping_add(which) as u64);
    NormalNoise::create_legacy_nether_biome(&mut random, -7, &[1.0, 1.0])
}

/// The base noise of the terrain (`old_blended_noise`) with the five numbers the data
/// gives it. A dimension with the old random numbers seeds it from the world's seed
/// itself, the others by the name `minecraft:terrain`.
#[must_use]
pub fn blended_noise(
    seed: i64,
    legacy_random_source: bool,
    splitter: &RandomSplitter,
    scales: [f64; 5],
) -> BlendedNoise {
    let [
        xz_scale,
        y_scale,
        xz_factor,
        y_factor,
        smear_scale_multiplier,
    ] = scales;
    if legacy_random_source {
        let mut random = LegacyRandom::from_seed(seed as u64);
        BlendedNoise::new(
            &mut random,
            xz_scale,
            y_scale,
            xz_factor,
            y_factor,
            smear_scale_multiplier,
        )
    } else {
        let mut random = splitter.with_hash_of(&NameHash::new("minecraft:terrain"));
        BlendedNoise::new(
            &mut random,
            xz_scale,
            y_scale,
            xz_factor,
            y_factor,
            smear_scale_multiplier,
        )
    }
}

/// A noise at a block position, its coordinates scaled.
#[inline]
#[must_use]
pub fn noise3(noise: &NormalNoise, x: i32, y: i32, z: i32, xz_scale: f64, y_scale: f64) -> f32 {
    noise.get(
        f64::from(x) * xz_scale,
        f64::from(y) * y_scale,
        f64::from(z) * xz_scale,
    )
}

/// A noise whose scale of y is zero: it is read flat.
#[inline]
#[must_use]
pub fn noise2(noise: &NormalNoise, x: i32, z: i32, xz_scale: f64) -> f32 {
    noise.get_xz(f64::from(x) * xz_scale, f64::from(z) * xz_scale)
}

/// A flat noise whose scaled coordinates are moved by two other functions' values.
#[inline]
#[must_use]
pub fn shifted_noise2(
    noise: &NormalNoise,
    x: i32,
    z: i32,
    xz_scale: f64,
    shift_x: f32,
    shift_z: f32,
) -> f32 {
    noise.get_xz(
        f64::from(x) * xz_scale + f64::from(shift_x),
        f64::from(z) * xz_scale + f64::from(shift_z),
    )
}

/// `shift_a`: the offset noise at a quarter of the position, times four.
#[inline]
#[must_use]
pub fn shift_a(noise: &NormalNoise, x: i32, z: i32) -> f32 {
    noise.get_xz(f64::from(x) * 0.25, f64::from(z) * 0.25) * 4.0
}

/// `shift_b`: the same noise read in another plane, so that the two shifts differ.
#[inline]
#[must_use]
pub fn shift_b(noise: &NormalNoise, x: i32, z: i32) -> f32 {
    noise.get_xy(f64::from(z) * 0.25, f64::from(x) * 0.25) * 4.0
}

#[inline]
#[must_use]
pub fn blended(noise: &BlendedNoise, x: i32, y: i32, z: i32) -> f32 {
    noise.get(f64::from(x), f64::from(y), f64::from(z))
}

/// `gradient` along an axis: `from_value` at and below `from`, `to_value` at and above
/// `to`, evenly between.
#[inline]
#[must_use]
pub fn gradient(coordinate: i32, from: i32, to: i32, from_value: f32, to_value: f32) -> f32 {
    let steps = coordinate.clamp(from, to) - from;
    from_value + steps as f32 * ((to_value - from_value) / (to - from) as f32)
}

/// The smaller of two values as Java's `Math.min` has it: minus zero is below zero.
#[inline]
#[must_use]
pub fn min(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        return a;
    }
    if a == 0.0 && b == 0.0 && b.is_sign_negative() {
        return b;
    }
    if a <= b { a } else { b }
}

/// The larger of two values as Java's `Math.max` has it: zero is above minus zero.
#[inline]
#[must_use]
pub fn max(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        return a;
    }
    if a == 0.0 && b == 0.0 && a.is_sign_negative() {
        return b;
    }
    if a >= b { a } else { b }
}

/// `clamp` as the game computes it: the lower bound where the value is below it,
/// otherwise the smaller of the value and the upper bound.
#[inline]
#[must_use]
pub fn clamp(value: f32, lower: f32, upper: f32) -> f32 {
    if value < lower {
        lower
    } else {
        min(value, upper)
    }
}

#[inline]
#[must_use]
pub fn abs(value: f32) -> f32 {
    value.abs()
}

#[inline]
#[must_use]
pub fn negate(value: f32) -> f32 {
    -value
}

#[inline]
#[must_use]
pub fn square(value: f32) -> f32 {
    value * value
}

#[inline]
#[must_use]
pub fn cube(value: f32) -> f32 {
    value * value * value
}

#[inline]
#[must_use]
pub fn half_negative(value: f32) -> f32 {
    if value > 0.0 { value } else { value * 0.5 }
}

#[inline]
#[must_use]
pub fn quarter_negative(value: f32) -> f32 {
    if value > 0.0 { value } else { value * 0.25 }
}

/// `squeeze`: the value held within one of zero, then bent so that it flattens
/// towards the ends.
#[inline]
#[must_use]
pub fn squeeze(value: f32) -> f32 {
    let held = clamp(value, -1.0, 1.0);
    held / 2.0 - held * held * held / 24.0
}

/// `lerp` where `alpha` is neither zero nor one. At zero the game takes `first` as it
/// is and at one `second`, without computing the other; the emitted code does the
/// same before it calls this.
#[inline]
#[must_use]
pub fn lerp(alpha: f32, first: f32, second: f32) -> f32 {
    first + alpha * (second - first)
}

/// `distance_to_point` with the Euclidean metric.
#[inline]
#[must_use]
pub fn distance(x: i32, y: i32, z: i32, point: [i32; 3]) -> f32 {
    let dx = point[0] as f32 - x as f32;
    let dy = point[1] as f32 - y as f32;
    let dz = point[2] as f32 - z as f32;
    libm::sqrtf(dx * dx + dy * dy + dz * dz)
}

/// `interpolated` at a single position: `corner` at the eight corners of the cell the
/// position is in, interpolated along x, then y, then z.
///
/// This is what the game gives when a function is asked at one position. When it
/// fills a volume it steps through a cell another way, and the bits are not always
/// these: of the overworld's final density over one chunk, the game's own two ways
/// differed in the last bit at a third of the blocks. Filling is the noise chunk's own
/// code, and has its own values to be compared with.
#[inline]
pub fn interpolate(
    cell_size_xz: i32,
    cell_size_y: i32,
    x: i32,
    y: i32,
    z: i32,
    corner: &mut impl FnMut(i32, i32, i32) -> f32,
) -> f32 {
    let x0 = x.div_euclid(cell_size_xz) * cell_size_xz;
    let y0 = y.div_euclid(cell_size_y) * cell_size_y;
    let z0 = z.div_euclid(cell_size_xz) * cell_size_xz;
    let (x1, y1, z1) = (
        x0.wrapping_add(cell_size_xz),
        y0.wrapping_add(cell_size_y),
        z0.wrapping_add(cell_size_xz),
    );
    let tx = (x - x0) as f32 / cell_size_xz as f32;
    let ty = (y - y0) as f32 / cell_size_y as f32;
    let tz = (z - z0) as f32 / cell_size_xz as f32;

    // The two heights of a column are asked one after the other, so that what the
    // column's flat functions remembered serves both.
    let c000 = corner(x0, y0, z0);
    let c010 = corner(x0, y1, z0);
    let c100 = corner(x1, y0, z0);
    let c110 = corner(x1, y1, z0);
    let c001 = corner(x0, y0, z1);
    let c011 = corner(x0, y1, z1);
    let c101 = corner(x1, y0, z1);
    let c111 = corner(x1, y1, z1);

    let near = lerp(ty, lerp(tx, c000, c100), lerp(tx, c010, c110));
    let far = lerp(ty, lerp(tx, c001, c101), lerp(tx, c011, c111));
    lerp(tz, near, far)
}

/// `find_top_surface`: the highest multiple of `cell_height` at or below
/// `upper_bound`, and not below `lower_bound`, at which `density` is positive; the
/// lower bound where there is none.
#[inline]
pub fn find_top_surface(
    upper_bound: f32,
    lower_bound: i32,
    cell_height: i32,
    density: &mut impl FnMut(i32) -> f32,
) -> f32 {
    let top = libm::floorf(upper_bound / cell_height as f32) as i32 * cell_height;
    let mut y = top;
    while y > lower_bound {
        if density(y) > 0.0 {
            return y as f32;
        }
        y -= cell_height;
    }
    lower_bound as f32
}

/// The outer islands of the End (`end_outer_islands`).
#[derive(Clone, Debug)]
pub struct EndIslands {
    noise: SimplexNoise,
}

impl EndIslands {
    /// An island is where the noise is below this.
    const THRESHOLD: f32 = -0.9;

    /// The islands of the world with this seed. The noise is seeded from the world's
    /// seed by `java.util.Random` after 17,292 numbers were drawn from it, in every
    /// dimension.
    #[must_use]
    pub fn new(seed: i64) -> Self {
        let mut random = LegacyRandom::from_seed(seed as u64);
        random.consume_count(17_292);
        Self {
            noise: SimplexNoise::new_without_offset(&mut random),
        }
    }

    /// The function at a block position; it does not change with y.
    #[must_use]
    pub fn sample(&self, x: i32, z: i32) -> f32 {
        // Java's division, which rounds towards zero.
        (self.height(x / 8, z / 8) - 8.0) / 128.0
    }

    /// How much land there is at a position counted in eighths of a chunk: the most
    /// that any island within twelve chunks gives, between -100 and 80.
    fn height(&self, section_x: i32, section_z: i32) -> f32 {
        let chunk_x = section_x / 2;
        let chunk_z = section_z / 2;
        let within_x = section_x % 2;
        let within_z = section_z % 2;

        let mut height = -100.0_f32;
        for offset_x in -12..=12_i32 {
            for offset_z in -12..=12_i32 {
                let island_x = i64::from(chunk_x) + i64::from(offset_x);
                let island_z = i64::from(chunk_z) + i64::from(offset_z);
                // No island within 64 chunks of the middle, where the main one is.
                if island_x * island_x + island_z * island_z > 4096
                    && self.noise.get_2d(island_x as f64, island_z as f64) < Self::THRESHOLD
                {
                    let size = ((island_x as f32).abs() * 3439.0 + (island_z as f32).abs() * 147.0)
                        % 13.0
                        + 9.0;
                    let dx = within_x as f32 - (offset_x * 2) as f32;
                    let dz = within_z as f32 - (offset_z * 2) as f32;
                    let from_this =
                        clamp(100.0 - libm::sqrtf(dx * dx + dz * dz) * size, -100.0, 80.0);
                    if from_this > height {
                        height = from_this;
                    }
                }
            }
        }
        height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(value: f32) -> u32 {
        value.to_bits()
    }

    #[test]
    fn minimum_and_maximum_tell_the_two_zeros_apart() {
        assert_eq!(bits(min(0.0, -0.0)), bits(-0.0));
        assert_eq!(bits(min(-0.0, 0.0)), bits(-0.0));
        assert_eq!(bits(max(0.0, -0.0)), bits(0.0));
        assert_eq!(bits(max(-0.0, 0.0)), bits(0.0));
        assert_eq!(min(1.0, 2.0), 1.0);
        assert_eq!(max(1.0, 2.0), 2.0);
        assert_eq!(min(-3.0, -3.5), -3.5);
        assert!(min(f32::NAN, 1.0).is_nan() && max(1.0, f32::NAN).is_nan());
    }

    #[test]
    fn clamp_gives_what_the_game_gives_at_the_zeros() {
        // The four cases the game's own classes were asked for.
        assert_eq!(bits(clamp(-0.0, 0.0, 1.0)), bits(-0.0));
        assert_eq!(bits(clamp(0.0, -0.0, 1.0)), bits(0.0));
        assert_eq!(bits(clamp(0.0, -1.0, -0.0)), bits(-0.0));
        assert_eq!(bits(clamp(-0.0, -1.0, 0.0)), bits(-0.0));
        assert_eq!(clamp(-2.0, -1.0, 1.0), -1.0);
        assert_eq!(clamp(2.0, -1.0, 1.0), 1.0);
        assert_eq!(clamp(0.25, -1.0, 1.0), 0.25);
    }

    #[test]
    fn the_small_steps_give_what_the_game_gives_at_minus_zero() {
        assert_eq!(bits(abs(-0.0)), bits(0.0));
        assert_eq!(bits(half_negative(-0.0)), bits(-0.0));
        assert_eq!(bits(quarter_negative(-0.0)), bits(-0.0));
        assert_eq!(bits(squeeze(-0.0)), bits(0.0));
        assert_eq!(bits(square(-0.0)), bits(0.0));
        assert_eq!(bits(cube(-0.0)), bits(-0.0));
        assert_eq!(bits(negate(0.0)), bits(-0.0));
        assert_eq!(half_negative(-2.0), -1.0);
        assert_eq!(quarter_negative(-2.0), -0.5);
        assert_eq!(quarter_negative(2.0), 2.0);
        assert_eq!(squeeze(5.0), 0.5 - 1.0 / 24.0);
    }

    #[test]
    fn a_gradient_stays_at_its_ends_outside_its_range() {
        assert_eq!(gradient(-100, -64, 320, 1.5, -1.5), 1.5);
        assert_eq!(gradient(320, -64, 320, 1.5, -1.5), -1.5);
        assert_eq!(gradient(9000, -64, 320, 1.5, -1.5), -1.5);
        assert_eq!(gradient(128, -64, 320, 1.5, -1.5), 0.0);
        assert_eq!(gradient(4, 0, 16, 0.0, 1.0), 0.25);
    }

    #[test]
    fn interpolation_is_the_corner_at_a_corner_and_between_them_inside() {
        // A function that is linear in each coordinate is given back exactly at the
        // corners and at the middle of a cell.
        let mut asked = Vec::new();
        let mut corner = |x: i32, y: i32, z: i32| {
            asked.push([x, y, z]);
            x as f32 + 10.0 * y as f32 + 100.0 * z as f32
        };
        assert_eq!(interpolate(4, 8, 4, 8, -4, &mut corner), 4.0 + 80.0 - 400.0);
        assert_eq!(
            interpolate(4, 8, 6, 12, -2, &mut corner),
            6.0 + 120.0 - 200.0
        );
        // Cells are counted downwards below zero: -1 is in the cell from -4 to 0.
        asked.clear();
        let mut corner = |x: i32, y: i32, z: i32| {
            asked.push([x, y, z]);
            0.0
        };
        let _ = interpolate(4, 8, -1, -1, -1, &mut corner);
        assert_eq!(asked[0], [-4, -8, -4]);
        assert_eq!(
            asked[1],
            [-4, 0, -4],
            "the two heights of a column in a row"
        );
        assert_eq!(asked[7], [0, 0, 0]);
        assert_eq!(asked.len(), 8);
    }

    #[test]
    fn the_top_surface_is_the_highest_cell_with_ground() {
        // Ground below 37: the highest multiple of 8 below it is 32.
        let mut asked = Vec::new();
        let mut density = |y: i32| {
            asked.push(y);
            if y < 37 { 1.0 } else { -1.0 }
        };
        assert_eq!(find_top_surface(100.5, -64, 8, &mut density), 32.0);
        assert_eq!(asked, [96, 88, 80, 72, 64, 56, 48, 40, 32]);
        // No ground at all, and an upper bound at or below the lower one.
        assert_eq!(find_top_surface(100.5, -64, 8, &mut |_| -1.0), -64.0);
        assert_eq!(find_top_surface(-64.0, -64, 8, &mut |_| 1.0), -64.0);
        assert_eq!(find_top_surface(-200.0, -64, 8, &mut |_| 1.0), -64.0);
        // A negative bound rounds down to its cell.
        assert_eq!(find_top_surface(-1.0, -64, 8, &mut |_| 1.0), -8.0);
    }

    static INNER: Spline = Spline {
        coordinate: 1,
        locations: &[0.0, 1.0],
        derivatives: &[0.0, 0.0],
        values: &[SplineValue::Constant(10.0), SplineValue::Constant(20.0)],
    };
    static OUTER: Spline = Spline {
        coordinate: 0,
        locations: &[-1.0, 1.0],
        derivatives: &[2.0, 3.0],
        values: &[SplineValue::Constant(1.0), SplineValue::Spline(&INNER)],
    };

    #[test]
    fn a_spline_goes_on_straight_outside_its_points_and_nests() {
        // Below the first point: its value, and its slope from there.
        let mut asked = Vec::new();
        let mut coordinate = |index: u16| {
            asked.push(index);
            if index == 0 { -2.0 } else { 0.5 }
        };
        assert_eq!(spline(&OUTER, &mut coordinate), 1.0 - 2.0);
        assert_eq!(asked, [0], "the nested spline is not looked at");

        // Above the last: the nested spline's value there, and the outer slope.
        let mut coordinate = |index: u16| if index == 0 { 3.0 } else { 0.0 };
        assert_eq!(spline(&OUTER, &mut coordinate), 10.0 + 3.0 * 2.0);

        // Between the two, and the nested spline halfway between its own.
        let mut coordinate = |index: u16| if index == 0 { 0.0 } else { 0.5 };
        let nested = spline(&INNER, &mut |_| 0.5);
        assert_eq!(nested, 15.0);
        assert_eq!(
            spline(&OUTER, &mut coordinate),
            hermite_interpolate(-1.0, 1.0, 1.0, 15.0, 2.0, 3.0, 0.0)
        );
    }

    #[test]
    fn memory_gives_back_only_the_value_of_the_position_it_was_given() {
        let mut memory = Memory::<2>::default();
        assert_eq!(
            memory.recall(0, 0, 0, 0),
            None,
            "nothing is remembered at first"
        );
        memory.remember(0, 1, 2, 3, 0.5);
        assert_eq!(memory.recall(0, 1, 2, 3), Some(0.5));
        assert_eq!(memory.recall(0, 1, 2, 4), None);
        assert_eq!(memory.recall(1, 1, 2, 3), None);
        memory.remember(0, 1, 2, 4, 0.75);
        assert_eq!(memory.recall(0, 1, 2, 3), None);
        assert_eq!(memory.recall(0, 1, 2, 4), Some(0.75));
        let _ = Memory::<0>::default();
    }
}
