// Adapted from the scalar paths of SteelMC's `steel-worldgen/src/noise/normal_noise.rs`.

//! The noises of the noise router, the game's `NormalNoise` as 26.3 samples it.

use crate::improved_noise::ImprovedNoise;
use crate::math::pow2;
use crate::random::{NameHash, PositionalRandom, Random, RandomSplitter};

/// The second stack of octaves is sampled at this multiple of the first one's
/// frequency, so that the two never line up.
const INPUT_FACTOR: f64 = 1.018_126_888_217_522_7;
const TARGET_DEVIATION: f64 = 1.0 / 3.0;
const DEVIATION_COEFFICIENT: f64 = 0.270_224_783_124_521_1;

/// One octave of one of the two stacks, with its frequency and its weight worked out
/// once. The sum over the layers in their order is the noise.
#[derive(Debug, Clone)]
struct Layer {
    noise: ImprovedNoise,
    frequency: f64,
    amplitude: f32,
}

/// A named noise of world generation: temperature, vegetation, continentalness,
/// erosion, the ridges, the noises of caves, aquifers and ore veins, the surface's.
/// A worldgen data file gives each a first octave and a list of amplitudes.
///
/// It is two stacks of Perlin octaves, the second at a slightly higher frequency,
/// scaled so that the sum has about the same spread whatever the octaves are. Since
/// 26.3 the sum is taken in single precision, layer by layer.
#[derive(Debug, Clone)]
pub struct NormalNoise {
    layers: Box<[Layer]>,
    max_value: f64,
}

impl NormalNoise {
    /// The noise with the given identifier (`minecraft:erosion`) from the world's
    /// factory, as the game's `RandomState` makes the noises of a dimension.
    #[must_use]
    pub fn create(
        splitter: &RandomSplitter,
        noise_id: &str,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let mut random = splitter.with_hash_of(&NameHash::new(noise_id));
        Self::create_from_random(&mut random, first_octave, amplitudes)
    }

    /// The noise from a generator, which advances by two factories. The octaves of
    /// each stack are seeded by name (`octave_-7`) from one of them.
    #[must_use]
    pub fn create_from_random<R: Random + ?Sized>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let base_amplitude = parity_base_amplitude(first_octave, amplitudes);
        Self::create_from_random_with_params(
            random,
            first_octave,
            base_amplitude,
            amplitudes.len() as i32,
            true,
            amplitudes,
        )
    }

    /// The noise of the Nether's biomes, whose octaves are made one after the other
    /// from the generator itself as before 1.18: octave zero first, then downwards.
    ///
    /// # Panics
    ///
    /// If there are no amplitudes or an octave would be above zero, which the game
    /// refuses as well.
    #[must_use]
    pub fn create_legacy_nether_biome<R: Random + ?Sized>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        assert!(!amplitudes.is_empty(), "a noise needs at least one octave");
        assert!(
            first_octave <= 0 && -first_octave >= amplitudes.len() as i32 - 1,
            "the old initialisation has no octaves above zero"
        );
        let base_amplitude = parity_base_amplitude(first_octave, amplitudes);
        let octaves = build_octaves(
            first_octave,
            base_amplitude,
            amplitudes.len() as i32,
            true,
            amplitudes,
        );
        let target_amplitude = target_amplitude(&octaves);
        let value_factor =
            (normalisation_factor(target_amplitude, &octaves) * base_amplitude) as f32;

        let mut layers = Vec::with_capacity(octaves.len() * 2);
        Self::append_legacy_layers(
            random,
            first_octave,
            amplitudes,
            1.0,
            value_factor,
            &mut layers,
        );
        Self::append_legacy_layers(
            random,
            first_octave,
            amplitudes,
            INPUT_FACTOR,
            value_factor,
            &mut layers,
        );
        let max_value = f64::from(
            layers
                .iter()
                .fold(0.0_f32, |value, layer| value + layer.amplitude.abs() * 2.0),
        );
        Self {
            layers: layers.into_boxed_slice(),
            max_value,
        }
    }

    /// One stack of the old initialisation, appended from the lowest octave up.
    fn append_legacy_layers<R: Random + ?Sized>(
        random: &mut R,
        first_octave: i32,
        amplitudes: &[f64],
        stack_frequency: f64,
        stack_amplitude: f32,
        layers: &mut Vec<Layer>,
    ) {
        let zero_index = first_octave.unsigned_abs() as usize;
        let mut noises = vec![None; amplitudes.len()];
        // The game makes octave zero even where it is not used.
        let zero_noise = ImprovedNoise::new(random);
        if zero_index < noises.len() && amplitudes[zero_index] != 0.0 {
            noises[zero_index] = Some(zero_noise);
        }
        for index in (0..zero_index).rev() {
            if index < noises.len() && amplitudes[index] != 0.0 {
                noises[index] = Some(ImprovedNoise::new(random));
            } else {
                random.consume_count(262);
            }
        }

        let octave_count = amplitudes.len() as i32;
        let mut frequency = pow2(first_octave);
        let mut amplitude = pow2(octave_count - 1) / (pow2(octave_count) - 1.0);
        for (noise, modifier) in noises.into_iter().zip(amplitudes) {
            if let Some(noise) = noise {
                layers.push(Layer {
                    noise,
                    frequency: frequency * stack_frequency,
                    amplitude: (amplitude * modifier) as f32 * stack_amplitude,
                });
            }
            frequency *= 2.0;
            amplitude /= 2.0;
        }
    }

    /// The noise as 26.3's data describes one: a base octave and amplitude, a number
    /// of octaves, whether the octaves' shares are scaled to sum to one, and a
    /// modifier per octave (one where the list ends).
    #[must_use]
    pub fn create_with_params(
        splitter: &RandomSplitter,
        noise_id: &str,
        base_octave: i32,
        base_amplitude: f64,
        octave_count: i32,
        normalise: bool,
        amplitude_modifiers: &[f64],
    ) -> Self {
        let mut random = splitter.with_hash_of(&NameHash::new(noise_id));
        Self::create_from_random_with_params(
            &mut random,
            base_octave,
            base_amplitude,
            octave_count,
            normalise,
            amplitude_modifiers,
        )
    }

    /// [`NormalNoise::create_with_params`] from a generator, which advances by two
    /// factories.
    #[must_use]
    pub fn create_from_random_with_params<R: Random + ?Sized>(
        random: &mut R,
        base_octave: i32,
        base_amplitude: f64,
        octave_count: i32,
        normalise: bool,
        amplitude_modifiers: &[f64],
    ) -> Self {
        let octaves = build_octaves(
            base_octave,
            base_amplitude,
            octave_count,
            normalise,
            amplitude_modifiers,
        );
        let target_amplitude = target_amplitude(&octaves);
        let normalisation_factor = normalisation_factor(target_amplitude, &octaves);

        let first_random = random.next_positional();
        let second_random = random.next_positional();
        let mut layers = Vec::with_capacity(octaves.len() * 2);
        for octave in octaves {
            let name = NameHash::new(&format!("octave_{}", octave.index));
            let mut first = first_random.with_hash_of(&name);
            let mut second = second_random.with_hash_of(&name);
            let amplitude = (normalisation_factor * octave.amplitude) as f32;
            layers.push(Layer {
                noise: ImprovedNoise::new(&mut first),
                frequency: octave.frequency,
                amplitude,
            });
            layers.push(Layer {
                noise: ImprovedNoise::new(&mut second),
                frequency: octave.frequency * INPUT_FACTOR,
                amplitude,
            });
        }

        Self {
            layers: layers.into_boxed_slice(),
            max_value: target_amplitude * TARGET_DEVIATION * 6.0,
        }
    }

    /// The noise at a point. The noise router asks for it at block coordinates scaled
    /// by the density function that holds the noise.
    #[inline]
    #[must_use]
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut value = 0.0_f32;
        for layer in &self.layers {
            let frequency = layer.frequency;
            value += layer.amplitude
                * layer
                    .noise
                    .noise(x * frequency, y * frequency, z * frequency);
        }
        value
    }

    /// The noise at `(x, 0, z)`, as the climate noises are sampled.
    #[inline]
    #[must_use]
    pub fn get_xz(&self, x: f64, z: f64) -> f32 {
        let mut value = 0.0_f32;
        for layer in &self.layers {
            let frequency = layer.frequency;
            value += layer.amplitude * layer.noise.noise_xz(x * frequency, z * frequency);
        }
        value
    }

    /// The noise at `(x, y, 0)`.
    #[inline]
    #[must_use]
    pub fn get_xy(&self, x: f64, y: f64) -> f32 {
        let mut value = 0.0_f32;
        for layer in &self.layers {
            let frequency = layer.frequency;
            value += layer.amplitude * layer.noise.noise_xy(x * frequency, y * frequency);
        }
        value
    }

    /// A bound of the noise's absolute value, which the density functions use to
    /// tell what a noise can never reach.
    #[inline]
    #[must_use]
    pub const fn max_value(&self) -> f64 {
        self.max_value
    }
}

/// An octave that takes part: its number, its frequency and its weight before the
/// two stacks are scaled together.
#[derive(Debug, Clone, Copy)]
struct Octave {
    index: i32,
    frequency: f64,
    amplitude: f64,
}

fn build_octaves(
    base_octave: i32,
    base_amplitude: f64,
    octave_count: i32,
    normalise: bool,
    amplitude_modifiers: &[f64],
) -> Vec<Octave> {
    let mut frequency = pow2(base_octave);
    let mut amplitude = base_amplitude;
    if normalise {
        amplitude *= normalisation_constant(octave_count);
    }

    let mut octaves = Vec::new();
    for index in 0..octave_count {
        let modifier = amplitude_modifiers
            .get(index as usize)
            .copied()
            .unwrap_or(1.0);
        if modifier != 0.0 {
            octaves.push(Octave {
                index: base_octave + index,
                frequency,
                amplitude: amplitude * modifier,
            });
        }
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    octaves
}

/// The share of the first of `octave_count` halving octaves, so that all sum to one.
fn normalisation_constant(octave_count: i32) -> f64 {
    pow2(octave_count - 1) / (pow2(octave_count) - 1.0)
}

fn target_amplitude(octaves: &[Octave]) -> f64 {
    let mut sum = 0.0;
    for octave in octaves {
        sum += octave.amplitude.abs();
    }
    sum
}

/// The factor that gives the sum of both stacks the deviation the game aims at.
fn normalisation_factor(target_amplitude: f64, octaves: &[Octave]) -> f64 {
    let mut variance = 0.0;
    for octave in octaves {
        let deviation = DEVIATION_COEFFICIENT * octave.amplitude.abs();
        variance += deviation * deviation;
    }
    if variance == 0.0 {
        return 0.0;
    }
    target_amplitude * TARGET_DEVIATION / (libm::sqrt(variance) * libm::sqrt(2.0))
}

/// The factor of the noise as it was described before 26.3: by the first and the
/// last octave that take part.
fn parity_normalisation_factor(base_amplitude: f64, amplitudes: &[f64]) -> f64 {
    let mut range = None;
    for (index, amplitude) in amplitudes.iter().enumerate() {
        if *amplitude != 0.0 {
            range = Some(match range {
                Some((first, _)) => (first, index as i32),
                None => (index as i32, index as i32),
            });
        }
    }
    let Some((first, last)) = range else {
        return 0.0;
    };
    base_amplitude * 0.5 * TARGET_DEVIATION / (0.1 * (1.0 + 1.0 / f64::from(last - first + 1)))
}

/// The base amplitude with which the description of 26.3 gives the noise that a first
/// octave and a list of amplitudes described before.
fn parity_base_amplitude(first_octave: i32, amplitudes: &[f64]) -> f64 {
    if amplitudes.is_empty() {
        return 1.0;
    }
    let octaves = build_octaves(first_octave, 1.0, amplitudes.len() as i32, true, amplitudes);
    let new_factor = normalisation_factor(target_amplitude(&octaves), &octaves);
    if new_factor == 0.0 {
        return 1.0;
    }
    parity_normalisation_factor(1.0, amplitudes) / new_factor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random::{LegacyRandom, RandomSource, Xoroshiro};

    #[test]
    fn the_planes_through_zero_are_the_full_noise_there() {
        // SteelMC's cases.
        let samples = [
            (0.0, -0.0),
            (1.25, -30.75),
            (-1000.0, 4096.5),
            (33_554_431.5, -33_554_432.25),
            (-0.000_000_1, 0.000_000_1),
        ];
        for seed in [0, 42, 98_765] {
            for amplitudes in [&[1.0, 0.0, 1.0, 1.0, 0.5][..], &[0.0; 5][..]] {
                let mut random = RandomSource::Legacy(LegacyRandom::from_seed(seed));
                let noise = NormalNoise::create_from_random(&mut random, -6, amplitudes);
                for (a, b) in samples {
                    assert_eq!(
                        noise.get_xz(a, b).to_bits(),
                        noise.get(a, 0.0, b).to_bits(),
                        "xz: seed {seed}, ({a}, {b})"
                    );
                    assert_eq!(
                        noise.get_xy(a, b).to_bits(),
                        noise.get(a, b, 0.0).to_bits(),
                        "xy: seed {seed}, ({a}, {b})"
                    );
                }
            }
        }
    }

    #[test]
    fn a_noise_without_octaves_is_zero_everywhere() {
        let mut random = Xoroshiro::from_seed(1);
        let noise = NormalNoise::create_from_random(&mut random, -6, &[0.0; 5]);
        assert_eq!(noise.get(1.0, 2.0, 3.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(noise.max_value(), 0.0);
    }

    #[test]
    fn the_layers_are_the_octaves_of_two_factories_by_name() {
        let mut random = Xoroshiro::from_seed(99);
        let noise = NormalNoise::create_from_random(&mut random, -3, &[1.0, 0.0, 1.0]);

        let mut random = Xoroshiro::from_seed(99);
        let first = random.next_positional();
        let second = random.next_positional();
        assert_eq!(noise.layers.len(), 4);
        for (pair, octave) in noise.layers.chunks(2).zip([-3, -1]) {
            let name = NameHash::new(&format!("octave_{octave}"));
            let expected_first = ImprovedNoise::new(&mut first.with_hash_of(&name));
            let expected_second = ImprovedNoise::new(&mut second.with_hash_of(&name));
            assert_eq!(
                pair[0].noise.x_offset().to_bits(),
                expected_first.x_offset().to_bits()
            );
            assert_eq!(
                pair[1].noise.x_offset().to_bits(),
                expected_second.x_offset().to_bits()
            );
            assert_eq!(pair[0].frequency, pow2(octave));
            assert_eq!(pair[1].frequency, pow2(octave) * INPUT_FACTOR);
            assert_eq!(pair[0].amplitude.to_bits(), pair[1].amplitude.to_bits());
        }
        // The lower octave weighs four times the one two octaves up.
        assert_eq!(noise.layers[0].amplitude, noise.layers[2].amplitude * 4.0);
    }

    #[test]
    fn a_named_noise_is_the_noise_of_the_generator_for_its_name() {
        let splitter = Xoroshiro::from_seed(13_579).next_positional();
        let named = NormalNoise::create(&splitter, "minecraft:erosion", -9, &[1.0, 1.0]);
        let mut random = splitter.with_hash_of(&NameHash::new("minecraft:erosion"));
        let direct = NormalNoise::create_from_random(&mut random, -9, &[1.0, 1.0]);
        assert_eq!(
            named.get(100.0, 0.0, -2000.0).to_bits(),
            direct.get(100.0, 0.0, -2000.0).to_bits()
        );
    }

    #[test]
    fn the_noise_stays_within_its_bound() {
        let splitter = Xoroshiro::from_seed(4).next_positional();
        let noise = NormalNoise::create(&splitter, "minecraft:temperature", -4, &[1.5, 0.0, 1.0]);
        let mut widest = 0.0_f32;
        for i in 0..500 {
            let value = noise.get(f64::from(i) * 7.3, f64::from(i) * 0.9, f64::from(i) * -11.1);
            widest = widest.max(value.abs());
        }
        assert!(widest > 0.1, "the noise hardly varies");
        assert!(f64::from(widest) <= noise.max_value());
    }

    #[test]
    #[should_panic(expected = "no octaves above zero")]
    fn the_old_initialisation_refuses_octaves_above_zero() {
        let mut random = LegacyRandom::from_seed(0);
        let _ = NormalNoise::create_legacy_nether_biome(&mut random, -1, &[1.0, 1.0, 1.0]);
    }
}
