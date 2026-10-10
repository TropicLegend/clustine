// Adapted from SteelMC's `steel-worldgen/src/noise/perlin_simplex_noise.rs`.

//! Octaves of simplex noise, the game's `PerlinSimplexNoise`. Despite the name there
//! is no Perlin noise in it.

use std::collections::BTreeSet;

use crate::math::pow2;
use crate::random::{LegacyRandom, Random};
use crate::simplex_noise::SimplexNoise;

/// A sum of [`SimplexNoise`] octaves over the plane.
///
/// Biomes have three of these, each made from a fixed seed: one varies the
/// temperature with which snow and ice are decided, one is the "frozen ocean" noise
/// that decides where a frozen ocean has ice, and one tints grass and foliage.
#[derive(Debug, Clone)]
pub struct PerlinSimplexNoise {
    /// The octaves from the highest frequency down, as the game keeps them; `None`
    /// where an octave between the first and the last is not asked for.
    noise_levels: Vec<Option<SimplexNoise>>,
    highest_frequency_input_factor: f64,
    highest_frequency_value_factor: f32,
}

impl PerlinSimplexNoise {
    /// The noise with the given octaves, each a power of two of the frequency.
    ///
    /// The order in which the generator is used is the game's: octave zero is always
    /// made first, whether asked for or not. The octaves below zero follow from the
    /// same generator, and one that is left out costs its 262 draws. The octaves
    /// above zero come from a legacy generator of their own, seeded with the value of
    /// octave zero at its own offset.
    ///
    /// # Panics
    ///
    /// If `octaves` is empty, which the game refuses as well.
    #[must_use]
    pub fn new<R: Random + ?Sized>(random: &mut R, octaves: &[i32]) -> Self {
        let octave_set: BTreeSet<i32> = octaves.iter().copied().collect();
        let (Some(&first_octave), Some(&last_octave)) = (octave_set.first(), octave_set.last())
        else {
            panic!("a noise needs at least one octave");
        };
        let total = (last_octave - first_octave + 1) as usize;
        // Index zero is the highest octave, so octave zero is at this index, which is
        // negative if every octave is below zero.
        let zero_index = last_octave;

        let zero_octave = SimplexNoise::new(random);
        let high_frequency_seed = (last_octave > 0).then(|| {
            let value = zero_octave.get_3d(
                zero_octave.x_offset(),
                zero_octave.y_offset(),
                zero_octave.z_offset(),
            );
            // The game multiplies by `Long.MAX_VALUE` as a float, which is 2^63.
            (f64::from(value) * 9.223_372_036_854_776e18) as i64
        });

        let mut noise_levels: Vec<Option<SimplexNoise>> = vec![None; total];
        if zero_index >= 0 && (zero_index as usize) < total && octave_set.contains(&0) {
            noise_levels[zero_index as usize] = Some(zero_octave);
        }

        let first_below_zero = (zero_index + 1).max(0) as usize;
        for (index, level) in noise_levels.iter_mut().enumerate().skip(first_below_zero) {
            if octave_set.contains(&(zero_index - index as i32)) {
                *level = Some(SimplexNoise::new(random));
            } else {
                random.consume_count(262);
            }
        }

        if let Some(seed) = high_frequency_seed {
            let mut high_frequency_random = LegacyRandom::from_seed(seed as u64);
            for index in (0..zero_index as usize).rev() {
                if octave_set.contains(&(zero_index - index as i32)) {
                    noise_levels[index] = Some(SimplexNoise::new(&mut high_frequency_random));
                } else {
                    high_frequency_random.consume_count(262);
                }
            }
        }

        Self {
            noise_levels,
            highest_frequency_input_factor: pow2(last_octave),
            highest_frequency_value_factor: (1.0 / (pow2(total as i32) - 1.0)) as f32,
        }
    }

    /// The noise at a point of the plane (`getValue(x, z, false)`: the octaves'
    /// offsets are not added).
    #[must_use]
    pub fn get(&self, x: f64, z: f64) -> f32 {
        let mut sum = 0.0_f32;
        let mut factor = self.highest_frequency_input_factor;
        let mut amplitude = self.highest_frequency_value_factor;
        for noise in &self.noise_levels {
            if let Some(noise) = noise {
                sum += amplitude * noise.get_2d(x * factor, z * factor);
            }
            factor /= 2.0;
            amplitude *= 2.0;
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_octave_zero_is_the_simplex_noise_itself() {
        let noise = PerlinSimplexNoise::new(&mut LegacyRandom::from_seed(2345), &[0]);
        let plain = SimplexNoise::new(&mut LegacyRandom::from_seed(2345));
        for (x, z) in [(0.5, 0.25), (-13.75, 8.5), (1000.125, -77.0)] {
            assert_eq!(noise.get(x, z).to_bits(), plain.get_2d(x, z).to_bits());
        }
    }

    #[test]
    fn octaves_below_zero_follow_from_the_same_generator_and_weigh_more() {
        let noise = PerlinSimplexNoise::new(&mut LegacyRandom::from_seed(3456), &[-2, -1, 0]);
        let mut random = LegacyRandom::from_seed(3456);
        let zero = SimplexNoise::new(&mut random);
        let minus_one = SimplexNoise::new(&mut random);
        let minus_two = SimplexNoise::new(&mut random);

        let (x, z) = (12.5, -40.25);
        let seventh = (1.0_f64 / 7.0) as f32;
        let expected = 0.0
            + seventh * zero.get_2d(x, z)
            + seventh * 2.0 * minus_one.get_2d(x / 2.0, z / 2.0)
            + seventh * 2.0 * 2.0 * minus_two.get_2d(x / 4.0, z / 4.0);
        assert_eq!(noise.get(x, z).to_bits(), expected.to_bits());
    }

    #[test]
    fn an_octave_left_out_costs_its_draws() {
        let noise = PerlinSimplexNoise::new(&mut LegacyRandom::from_seed(5), &[-2, 0]);
        let mut random = LegacyRandom::from_seed(5);
        let zero = SimplexNoise::new(&mut random);
        random.consume_count(262);
        let minus_two = SimplexNoise::new(&mut random);

        let (x, z) = (3.5, 9.75);
        let seventh = (1.0_f64 / 7.0) as f32;
        let expected = 0.0
            + seventh * zero.get_2d(x, z)
            + seventh * 2.0 * 2.0 * minus_two.get_2d(x / 4.0, z / 4.0);
        assert_eq!(noise.get(x, z).to_bits(), expected.to_bits());
    }

    #[test]
    fn octaves_above_zero_come_from_a_generator_seeded_by_octave_zero() {
        let noise = PerlinSimplexNoise::new(&mut LegacyRandom::from_seed(11), &[0, 1]);
        let zero = SimplexNoise::new(&mut LegacyRandom::from_seed(11));
        let value = zero.get_3d(zero.x_offset(), zero.y_offset(), zero.z_offset());
        let seed = (f64::from(value) * 9.223_372_036_854_776e18) as i64;
        let one = SimplexNoise::new(&mut LegacyRandom::from_seed(seed as u64));

        let (x, z) = (3.5, 9.75);
        let third = (1.0_f64 / 3.0) as f32;
        let expected = 0.0 + third * one.get_2d(x * 2.0, z * 2.0) + third * 2.0 * zero.get_2d(x, z);
        assert_eq!(noise.get(x, z).to_bits(), expected.to_bits());
    }

    #[test]
    #[should_panic(expected = "at least one octave")]
    fn a_noise_without_octaves_is_refused() {
        let _ = PerlinSimplexNoise::new(&mut LegacyRandom::from_seed(0), &[]);
    }
}
