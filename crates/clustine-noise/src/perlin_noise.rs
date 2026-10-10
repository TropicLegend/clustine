// Adapted from the scalar paths of SteelMC's `steel-worldgen/src/noise/perlin_noise.rs`.

//! Octaves of Perlin noise summed in double precision: the game's `PerlinNoise` as it
//! was before 26.3 gave the noise router its own stacks.

use crate::improved_noise::ImprovedNoise;
use crate::math::{pow2, wrap};
use crate::random::{NameHash, PositionalRandom, Random, RandomSplitter};

/// A sum of [`ImprovedNoise`] octaves, each at twice the frequency and half the
/// weight of the one before, times an amplitude per octave.
///
/// The noises of the noise router are [`crate::NormalNoise`] and
/// [`crate::BlendedNoise`], which hold their octaves themselves. This one is for what
/// is still described as a first octave and a list of amplitudes and summed in double
/// precision, such as the surface's older noises.
#[derive(Debug, Clone)]
pub struct PerlinNoise {
    /// The octaves from the lowest frequency up; `None` where the amplitude is zero.
    noise_levels: Vec<Option<ImprovedNoise>>,
    amplitudes: Vec<f64>,
    /// The octaves that are there, with their factors worked out once.
    active_octaves: Vec<ActiveOctave>,
    lowest_frequency_value_factor: f64,
    max_value: f64,
}

#[derive(Debug, Clone)]
struct ActiveOctave {
    noise: ImprovedNoise,
    /// By what the coordinates are multiplied for this octave.
    input_factor: f64,
    /// The octave's amplitude times its share of the sum.
    output_factor: f64,
}

impl PerlinNoise {
    /// The noise whose octaves are seeded by name (`octave_-7` and so on) from a
    /// factory. `first_octave` is the lowest frequency as a power of two.
    #[must_use]
    pub fn create(splitter: &RandomSplitter, first_octave: i32, amplitudes: &[f64]) -> Self {
        let mut noise_levels = vec![None; amplitudes.len()];
        for (i, level) in noise_levels.iter_mut().enumerate() {
            if amplitudes[i] != 0.0 {
                let name = format!("octave_{}", first_octave + i as i32);
                let mut octave_random = splitter.with_hash_of(&NameHash::new(&name));
                *level = Some(ImprovedNoise::new(&mut octave_random));
            }
        }
        Self::from_parts(noise_levels, amplitudes, first_octave)
    }

    /// The noise as the game makes it from a generator today: a factory is taken from
    /// the generator, which advances by two longs, and the octaves are seeded by name
    /// from the factory. Two noises made one after the other from one generator
    /// therefore differ.
    #[must_use]
    pub fn create_from_random<R: Random + ?Sized>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let splitter = random.next_positional();
        Self::create(&splitter, first_octave, amplitudes)
    }

    /// The noise as the game made it before 1.18, which the Nether's biomes still
    /// use: the octaves are made one after the other from the generator itself, from
    /// octave zero downwards, and an octave that is left out still costs its 262
    /// draws.
    ///
    /// # Panics
    ///
    /// If `first_octave` is positive, which the game refuses as well.
    #[must_use]
    pub fn create_legacy_for_nether<R: Random + ?Sized>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        assert!(
            first_octave <= 0,
            "the old initialisation has no octaves above zero"
        );
        let octaves = amplitudes.len();
        let zero_octave_index = first_octave.unsigned_abs() as usize;
        let mut noise_levels = vec![None; octaves];
        for index in (0..=zero_octave_index).rev() {
            if index < octaves && amplitudes[index] != 0.0 {
                noise_levels[index] = Some(ImprovedNoise::new(random));
            } else {
                random.consume_count(262);
            }
        }
        Self::from_parts(noise_levels, amplitudes, first_octave)
    }

    fn from_parts(
        noise_levels: Vec<Option<ImprovedNoise>>,
        amplitudes: &[f64],
        first_octave: i32,
    ) -> Self {
        let octaves = amplitudes.len() as i32;
        let lowest_frequency_input_factor = pow2(first_octave);
        // The shares of the octaves halve and sum to one.
        let lowest_frequency_value_factor = pow2(octaves - 1) / (pow2(octaves) - 1.0);
        let max_value = Self::edge_value(amplitudes, lowest_frequency_value_factor, 2.0);

        let mut active_octaves = Vec::with_capacity(noise_levels.len());
        let mut input_factor = lowest_frequency_input_factor;
        let mut value_factor = lowest_frequency_value_factor;
        for (noise, amplitude) in noise_levels.iter().zip(amplitudes) {
            if let Some(noise) = noise {
                active_octaves.push(ActiveOctave {
                    noise: noise.clone(),
                    input_factor,
                    output_factor: amplitude * value_factor,
                });
            }
            input_factor *= 2.0;
            value_factor /= 2.0;
        }

        Self {
            noise_levels,
            amplitudes: amplitudes.to_vec(),
            active_octaves,
            lowest_frequency_value_factor,
            max_value,
        }
    }

    /// The sum if every octave gave `noise_value`.
    fn edge_value(amplitudes: &[f64], lowest_frequency_value_factor: f64, noise_value: f64) -> f64 {
        let mut value = 0.0;
        let mut value_factor = lowest_frequency_value_factor;
        for &amplitude in amplitudes {
            if amplitude != 0.0 {
                value += amplitude * noise_value * value_factor;
            }
            value_factor /= 2.0;
        }
        value
    }

    /// The noise at a point.
    #[inline]
    #[must_use]
    pub fn get(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut value = 0.0;
        for octave in &self.active_octaves {
            let factor = octave.input_factor;
            let noise = octave
                .noise
                .noise(wrap(x * factor), wrap(y * factor), wrap(z * factor));
            value += octave.output_factor * f64::from(noise);
        }
        value
    }

    /// The noise at `(x, 0, z)`.
    #[inline]
    #[must_use]
    pub fn get_xz(&self, x: f64, z: f64) -> f64 {
        let mut value = 0.0;
        for octave in &self.active_octaves {
            let factor = octave.input_factor;
            let noise = octave.noise.noise_xz(wrap(x * factor), wrap(z * factor));
            value += octave.output_factor * f64::from(noise);
        }
        value
    }

    /// The noise at `(x, y, 0)`.
    #[inline]
    #[must_use]
    pub fn get_xy(&self, x: f64, y: f64) -> f64 {
        let mut value = 0.0;
        for octave in &self.active_octaves {
            let factor = octave.input_factor;
            let noise = octave.noise.noise_xy(wrap(x * factor), wrap(y * factor));
            value += octave.output_factor * f64::from(noise);
        }
        value
    }

    /// The noise at a point with the older vertical smearing (see
    /// [`ImprovedNoise::noise_with_y_scale`]), summed in single precision. With
    /// `y_flat_hack` every octave is sampled at its own lattice's zero height, which
    /// is how the old noises of two dimensions were made.
    #[must_use]
    pub fn get_with_y_params(
        &self,
        x: f64,
        y: f64,
        z: f64,
        y_scale: f64,
        y_fudge: f64,
        y_flat_hack: bool,
    ) -> f32 {
        let mut value = 0.0_f32;
        for octave in &self.active_octaves {
            let factor = octave.input_factor;
            let noise = &octave.noise;
            let octave_y = if y_flat_hack {
                -noise.y_offset()
            } else {
                wrap(y * factor)
            };
            let sample = noise.noise_with_y_scale(
                wrap(x * factor),
                octave_y,
                wrap(z * factor),
                y_scale * factor,
                y_fudge * factor,
            );
            value += (octave.output_factor as f32) * sample;
        }
        value
    }

    /// A bound of the noise's absolute value.
    #[inline]
    #[must_use]
    pub const fn max_value(&self) -> f64 {
        self.max_value
    }

    /// The bound the game claims for the smeared noise (`PerlinNoise.maxBrokenValue`).
    /// It is called broken in the game too.
    #[must_use]
    pub fn max_broken_value(&self, y_scale: f64) -> f64 {
        Self::edge_value(
            &self.amplitudes,
            self.lowest_frequency_value_factor,
            y_scale + 2.0,
        )
    }

    /// An octave counted from the highest frequency, which is octave zero here, or
    /// `None` if its amplitude is zero or there is no such octave
    /// (`PerlinNoise.getOctaveNoise`).
    #[must_use]
    pub fn octave_noise(&self, i: usize) -> Option<&ImprovedNoise> {
        let index = self.noise_levels.len().checked_sub(i + 1)?;
        self.noise_levels[index].as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random::{LegacyRandom, Xoroshiro};

    fn splitter(seed: u64) -> RandomSplitter {
        Xoroshiro::from_seed(seed).next_positional()
    }

    #[test]
    fn the_same_factory_gives_the_same_noise() {
        let splitter = splitter(12_345);
        let first = PerlinNoise::create(&splitter, -3, &[1.0, 1.0, 1.0]);
        let second = PerlinNoise::create(&splitter, -3, &[1.0, 1.0, 1.0]);
        assert_eq!(
            first.get(100.0, 64.0, 100.0).to_bits(),
            second.get(100.0, 64.0, 100.0).to_bits()
        );
    }

    #[test]
    fn the_sum_is_the_octaves_at_doubling_frequency_and_halving_weight() {
        let splitter = splitter(5);
        let noise = PerlinNoise::create(&splitter, -2, &[1.0, 0.0, 3.0]);
        assert!(noise.octave_noise(1).is_none());
        assert!(noise.octave_noise(3).is_none());
        let lowest = noise
            .octave_noise(2)
            .expect("the first amplitude is not zero");
        let highest = noise
            .octave_noise(0)
            .expect("the last amplitude is not zero");

        // Three octaves share the sum as 4/7, 2/7 and 1/7.
        let (x, y, z) = (10.5, -3.25, 77.0);
        let lowest_share: f64 = 4.0 / 7.0;
        let highest_share: f64 = lowest_share / 2.0 / 2.0;
        let expected = 0.0
            + lowest_share * f64::from(lowest.noise(x * 0.25, y * 0.25, z * 0.25))
            + 3.0 * highest_share * f64::from(highest.noise(x, y, z));
        assert_eq!(noise.get(x, y, z).to_bits(), expected.to_bits());

        let bound: f64 = 0.0 + 2.0 * lowest_share + 3.0 * 2.0 * highest_share;
        assert_eq!(noise.max_value().to_bits(), bound.to_bits());
        let broken_bound: f64 = 0.0 + 3.0 * lowest_share + 3.0 * 3.0 * highest_share;
        assert_eq!(
            noise.max_broken_value(1.0).to_bits(),
            broken_bound.to_bits()
        );
    }

    #[test]
    fn octaves_are_seeded_by_their_names() {
        let splitter = splitter(5);
        let noise = PerlinNoise::create(&splitter, -2, &[1.0, 1.0, 1.0]);
        let mut random = splitter.with_hash_of(&NameHash::new("octave_-1"));
        let expected = ImprovedNoise::new(&mut random);
        let middle = noise.octave_noise(1).expect("the octave is there");
        assert_eq!(middle.x_offset().to_bits(), expected.x_offset().to_bits());
        assert_eq!(
            middle.noise(1.5, 2.5, 3.5).to_bits(),
            expected.noise(1.5, 2.5, 3.5).to_bits()
        );
    }

    #[test]
    fn the_sum_without_smearing_is_nearly_the_single_precision_one() {
        // SteelMC's cases: the two differ only in the width they are summed in.
        let noise = PerlinNoise::create(&splitter(12_345), -4, &[1.0, 0.0, 1.0, 1.0]);
        for (x, y, z) in [
            (0.0, 0.0, 0.0),
            (100.0, 64.0, -100.0),
            (-4096.25, -32.5, 1024.75),
        ] {
            let double = noise.get(x, y, z);
            let single = f64::from(noise.get_with_y_params(x, y, z, 0.0, 0.0, false));
            assert!((double - single).abs() < 1e-6, "{double} against {single}");
        }
    }

    #[test]
    fn the_flat_hack_samples_every_octave_at_its_own_zero() {
        let noise = PerlinNoise::create(&splitter(8), -3, &[1.0, 1.0]);
        let flat = noise.get_with_y_params(3.5, 1000.0, -9.25, 0.0, 0.0, true);
        let elsewhere = noise.get_with_y_params(3.5, -55.0, -9.25, 0.0, 0.0, true);
        assert_eq!(flat.to_bits(), elsewhere.to_bits());
    }

    #[test]
    fn two_noises_from_one_generator_differ() {
        let mut random = splitter(12_345).with_hash_of(&NameHash::new("test_noise"));
        let first = PerlinNoise::create_from_random(&mut random, -3, &[1.0, 1.0, 1.0]);
        let second = PerlinNoise::create_from_random(&mut random, -3, &[1.0, 1.0, 1.0]);
        let difference = first.get(100.0, 64.0, 100.0) - second.get(100.0, 64.0, 100.0);
        assert!(difference.abs() > 0.001, "{difference}");
    }

    #[test]
    fn the_old_initialisation_makes_octaves_from_zero_downwards() {
        let amplitudes = [1.0, 0.0, 1.0];
        let mut random = LegacyRandom::from_seed(77);
        let noise = PerlinNoise::create_legacy_for_nether(&mut random, -3, &amplitudes);

        // Octave zero is beyond the list and costs its draws; then -1 (index 2),
        // -2 (index 1, left out) and -3 (index 0).
        let mut expected = LegacyRandom::from_seed(77);
        expected.consume_count(262);
        let highest = ImprovedNoise::new(&mut expected);
        expected.consume_count(262);
        let lowest = ImprovedNoise::new(&mut expected);
        assert_eq!(random.state(), expected.state());

        let made_highest = noise.octave_noise(0).expect("the octave is there");
        let made_lowest = noise.octave_noise(2).expect("the octave is there");
        assert_eq!(
            made_highest.x_offset().to_bits(),
            highest.x_offset().to_bits()
        );
        assert_eq!(
            made_lowest.x_offset().to_bits(),
            lowest.x_offset().to_bits()
        );
    }

    #[test]
    fn the_planes_through_zero_are_the_full_noise_there() {
        // SteelMC's cases, compared by bits.
        let noise = PerlinNoise::create(&splitter(98_765), -6, &[1.0, 0.0, 1.0, 1.0, 0.5]);
        let samples = [
            (0.0, 0.0),
            (1.25, -30.75),
            (-1000.0, 4096.5),
            (33_554_431.5, -33_554_432.25),
            (-0.000_000_1, 0.000_000_1),
        ];
        for (a, b) in samples {
            assert_eq!(noise.get_xz(a, b).to_bits(), noise.get(a, 0.0, b).to_bits());
            assert_eq!(noise.get_xy(a, b).to_bits(), noise.get(a, b, 0.0).to_bits());
        }
    }

    #[test]
    fn the_noise_varies_from_place_to_place() {
        let noise = PerlinNoise::create(&splitter(42), -4, &[1.0, 1.0, 1.0, 1.0]);
        let values: Vec<f64> = (0..10)
            .map(|i| noise.get(f64::from(i) * 50.0, 64.0, f64::from(i) * 50.0))
            .collect();
        let lowest = values.iter().copied().fold(f64::INFINITY, f64::min);
        let highest = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(highest - lowest > 0.01);
        assert!(highest <= noise.max_value() && lowest >= -noise.max_value());
    }
}
