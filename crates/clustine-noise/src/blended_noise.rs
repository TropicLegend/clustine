// Adapted from the scalar paths of SteelMC's `steel-worldgen/src/noise/blended_noise.rs`.

//! The terrain noise that is as old as the game's infinite worlds, the game's
//! `BlendedNoise` as 26.3 samples it.

use crate::improved_noise::ImprovedNoise;
use crate::math::{clamp, clamped_lerp, pow2};
use crate::random::Random;

/// The scale every coordinate is multiplied by, from the days of the first terrain.
const BASE_SCALE: f64 = 684.412;

#[derive(Debug, Clone)]
struct SmearedLayer {
    noise: ImprovedNoise,
    frequency: f64,
    amplitude: f32,
    fudge_y_scale: f64,
}

/// Octaves of smeared Perlin noise, kept from the highest frequency down.
#[derive(Debug, Clone)]
struct SmearedStack {
    layers: Box<[SmearedLayer]>,
}

impl SmearedStack {
    /// A stack of the octaves from zero down to `first_octave`, made in that order
    /// from the generator, which each costs 262 draws of.
    fn create<R: Random + ?Sized>(
        random: &mut R,
        first_octave: i32,
        smear_y_scale: f64,
        value_factor: f64,
    ) -> Self {
        let octaves = -first_octave + 1;
        let mut value_factor = value_factor / (pow2(octaves) - 1.0);
        let mut frequency = 1.0;
        let mut layers = Vec::new();
        for _ in 0..octaves {
            layers.push(SmearedLayer {
                noise: ImprovedNoise::new(random),
                frequency,
                amplitude: value_factor as f32,
                fudge_y_scale: smear_y_scale * frequency,
            });
            frequency *= 0.5;
            value_factor *= 2.0;
        }
        Self {
            layers: layers.into_boxed_slice(),
        }
    }

    #[inline]
    fn sample(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut value = 0.0_f32;
        for layer in &self.layers {
            let frequency = layer.frequency;
            value += layer.amplitude
                * layer.noise.smeared_noise(
                    x * frequency,
                    y * frequency,
                    z * frequency,
                    layer.fudge_y_scale,
                );
        }
        value
    }
}

/// The base of the overworld's and the Nether's terrain (`minecraft:old_blended_noise`
/// in a noise router): two noises of sixteen octaves, and a third of eight octaves
/// that says how much of each to take.
///
/// The three stacks are made from a legacy generator whatever the dimension's
/// settings say; the game seeds it with the world's seed for the overworld.
#[derive(Debug, Clone)]
pub struct BlendedNoise {
    min_limit_noise: SmearedStack,
    max_limit_noise: SmearedStack,
    main_noise: SmearedStack,
    xz_multiplier: f64,
    y_multiplier: f64,
    main_xz_scale: f64,
    main_y_scale: f64,
}

impl BlendedNoise {
    /// The noise with the five numbers a noise router gives it. The overworld's are
    /// 0.25, 0.125, 80, 160 and 8.
    #[must_use]
    pub fn new<R: Random + ?Sized>(
        random: &mut R,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    ) -> Self {
        let xz_multiplier = BASE_SCALE * xz_scale;
        let y_multiplier = BASE_SCALE * y_scale;
        let limit_smear_scale_y = y_multiplier * smear_scale_multiplier;
        let main_smear_scale_y = limit_smear_scale_y / y_factor;
        // The game's factor of the two limits is a float literal.
        let limit_value_factor = f64::from(0.999_984_74_f32);

        let min_limit_noise =
            SmearedStack::create(random, -15, limit_smear_scale_y, limit_value_factor);
        let max_limit_noise =
            SmearedStack::create(random, -15, limit_smear_scale_y, limit_value_factor);
        let main_noise = SmearedStack::create(random, -7, main_smear_scale_y, 12.75);
        Self {
            min_limit_noise,
            max_limit_noise,
            main_noise,
            xz_multiplier,
            y_multiplier,
            main_xz_scale: xz_multiplier / xz_factor,
            main_y_scale: y_multiplier / y_factor,
        }
    }

    /// The noise at a block position (`BlendedNoise.compute`).
    #[inline]
    #[must_use]
    pub fn get(&self, block_x: f64, block_y: f64, block_z: f64) -> f32 {
        let limit_x = block_x * self.xz_multiplier;
        let limit_y = block_y * self.y_multiplier;
        let limit_z = block_z * self.xz_multiplier;
        let main = self.main_noise.sample(
            block_x * self.main_xz_scale,
            block_y * self.main_y_scale,
            block_z * self.main_xz_scale,
        );
        // Where the third noise says "all of one", the game returns that one as it is
        // and never samples the other.
        let alpha = clamp(main + 0.5_f32, 0.0, 1.0);
        if alpha == 0.0 {
            return self.min_limit_noise.sample(limit_x, limit_y, limit_z);
        }
        if alpha == 1.0 {
            return self.max_limit_noise.sample(limit_x, limit_y, limit_z);
        }
        let minimum = self.min_limit_noise.sample(limit_x, limit_y, limit_z);
        let maximum = self.max_limit_noise.sample(limit_x, limit_y, limit_z);
        clamped_lerp(minimum, maximum, alpha)
    }

    /// The noise at the given heights of one column, into `out`. As many values are
    /// written as the shorter of the two slices has.
    pub fn get_column(&self, block_x: i32, block_ys: &[i32], block_z: i32, out: &mut [f32]) {
        for (&block_y, value) in block_ys.iter().zip(out) {
            *value = self.get(f64::from(block_x), f64::from(block_y), f64::from(block_z));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random::{LegacyRandom, Xoroshiro};

    fn overworld(seed: u64) -> BlendedNoise {
        BlendedNoise::new(
            &mut LegacyRandom::from_seed(seed),
            0.25,
            0.125,
            80.0,
            160.0,
            8.0,
        )
    }

    #[test]
    fn two_values_are_the_games() {
        // SteelMC's test of the same name in substance: values of the game for seed
        // zero, the second far enough out for the coordinates to be wrapped.
        let noise = overworld(0);
        for (x, y, z, expected) in [
            (0.0, -56.0, -10_000.0, 0.410_753_88_f32),
            (20_000_068.0, 296.0, -19_999_796.0, 0.013_388_243_f32),
        ] {
            assert_eq!(noise.get(x, y, z).to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn making_the_noise_draws_forty_octaves_of_a_legacy_generator() {
        let mut used = LegacyRandom::from_seed(5);
        let _ = BlendedNoise::new(&mut used, 0.25, 0.125, 80.0, 160.0, 8.0);
        let mut skipped = LegacyRandom::from_seed(5);
        skipped.consume_count(262 * (16 + 16 + 8));
        assert_eq!(used.state(), skipped.state());
    }

    #[test]
    fn the_stacks_run_from_the_highest_frequency_down_with_doubling_weight() {
        let noise = overworld(0);
        let layers = &noise.main_noise.layers;
        assert_eq!(layers.len(), 8);
        assert_eq!(noise.min_limit_noise.layers.len(), 16);
        assert_eq!(layers[0].frequency, 1.0);
        assert_eq!(layers[7].frequency, 1.0 / 128.0);
        assert_eq!(layers[0].amplitude, (12.75_f64 / 255.0) as f32);
        assert_eq!(layers[7].amplitude, layers[0].amplitude * 128.0);
        assert_eq!(layers[3].fudge_y_scale, layers[0].fudge_y_scale / 8.0);
    }

    #[test]
    fn all_three_cases_of_the_blend_are_met_and_are_what_they_say() {
        let mut saw_minimum = false;
        let mut saw_maximum = false;
        let mut saw_between = false;
        for seed in [0, 42, 13_579] {
            let noise = overworld(seed);
            for x in [-20_000_068.0, -128.0, 0.0, 128.0, 20_000_068.0] {
                for z in [-19_999_796.0, -256.0, 0.0, 256.0, 19_999_796.0] {
                    for y in (-64..320).step_by(8) {
                        let y = f64::from(y);
                        let main = noise.main_noise.sample(
                            x * noise.main_xz_scale,
                            y * noise.main_y_scale,
                            z * noise.main_xz_scale,
                        );
                        let limits = (
                            x * noise.xz_multiplier,
                            y * noise.y_multiplier,
                            z * noise.xz_multiplier,
                        );
                        let minimum = noise.min_limit_noise.sample(limits.0, limits.1, limits.2);
                        let maximum = noise.max_limit_noise.sample(limits.0, limits.1, limits.2);
                        let value = noise.get(x, y, z);
                        if main <= -0.5 {
                            saw_minimum = true;
                            assert_eq!(value.to_bits(), minimum.to_bits());
                        } else if main >= 0.5 {
                            saw_maximum = true;
                            assert_eq!(value.to_bits(), maximum.to_bits());
                        } else {
                            saw_between = true;
                            let low = minimum.min(maximum);
                            let high = minimum.max(maximum);
                            assert!((low..=high).contains(&value));
                        }
                    }
                }
            }
        }
        assert!(saw_minimum && saw_maximum && saw_between);
    }

    #[test]
    fn a_column_is_its_blocks_one_by_one() {
        let noise = overworld(0);
        let ys = [
            -64, -56, -48, -40, -32, -24, -16, -8, 0, 8, 16, 24, 32, 40, 48, 56, 64,
        ];
        let mut column = [f32::NAN; 17];
        for length in 0..=ys.len() {
            noise.get_column(20_000_068, &ys, -19_999_796, &mut column[..length]);
            for (&y, &value) in ys.iter().zip(&column[..length]) {
                let single = noise.get(20_000_068.0, f64::from(y), -19_999_796.0);
                assert_eq!(value.to_bits(), single.to_bits(), "length {length}, y {y}");
            }
        }
    }

    #[test]
    fn the_noise_takes_any_generator() {
        let from_xoroshiro =
            BlendedNoise::new(&mut Xoroshiro::from_seed(0), 0.25, 0.125, 80.0, 160.0, 8.0);
        assert_ne!(
            from_xoroshiro.get(0.0, 64.0, 0.0).to_bits(),
            overworld(0).get(0.0, 64.0, 0.0).to_bits()
        );
    }
}
