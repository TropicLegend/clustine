// Adapted from the scalar paths of SteelMC's `steel-worldgen/src/noise/improved_noise.rs`.

//! One octave of Perlin's improved noise, as 26.3 computes it.
//!
//! Since 26.3 the game keeps coordinates and the octave's offsets in double precision
//! but takes the position within a lattice cell, the gradients and the interpolation
//! in single precision. Every operation below is in the width and the order the game
//! has it in; that is all there is to being equal to it.

use crate::math::{
    GRADIENT_F32, floor, grad_dot, lerp, lerp2, lerp3, smoothstep, smoothstep_derivative, wrap,
};
use crate::random::Random;

/// One octave of Perlin noise: a shuffled table of 256 bytes that chooses a gradient
/// for every lattice point, and an offset that moves the lattice.
///
/// Every noise of the noise router is a sum of these ([`crate::NormalNoise`],
/// [`crate::BlendedNoise`]), as are the older noises of the surface
/// ([`crate::PerlinNoise`]).
#[derive(Debug, Clone)]
pub struct ImprovedNoise {
    permutation: [u8; 256],
    x_offset: f64,
    y_offset: f64,
    z_offset: f64,
    y_offset_floor: i32,
    y_offset_fraction: f64,
    z_offset_floor: i32,
    z_offset_fraction: f64,
}

impl ImprovedNoise {
    /// An octave from a generator, which is advanced by three doubles for the offset
    /// and 256 bounded integers for the shuffle: 262 draws of a legacy generator.
    pub fn new<R: Random + ?Sized>(random: &mut R) -> Self {
        let x_offset = random.next_f64() * 256.0;
        let y_offset = random.next_f64() * 256.0;
        let z_offset = random.next_f64() * 256.0;

        let mut permutation = [0_u8; 256];
        for (i, entry) in permutation.iter_mut().enumerate() {
            *entry = i as u8;
        }
        for i in 0..256 {
            let offset = random.next_i32_bounded((256 - i) as i32) as usize;
            permutation.swap(i, i + offset);
        }

        let y_offset_floor = floor(y_offset);
        let z_offset_floor = floor(z_offset);
        Self {
            permutation,
            x_offset,
            y_offset,
            z_offset,
            y_offset_floor,
            y_offset_fraction: y_offset - f64::from(y_offset_floor),
            z_offset_floor,
            z_offset_fraction: z_offset - f64::from(z_offset_floor),
        }
    }

    /// How far the lattice is moved along x.
    #[must_use]
    pub const fn x_offset(&self) -> f64 {
        self.x_offset
    }

    /// How far the lattice is moved along y. The old "flat" noises sample at minus
    /// this, which is the lattice's own zero.
    #[must_use]
    pub const fn y_offset(&self) -> f64 {
        self.y_offset
    }

    /// How far the lattice is moved along z.
    #[must_use]
    pub const fn z_offset(&self) -> f64 {
        self.z_offset
    }

    /// The noise at a point (`PerlinNoise.noise` of 26.3), roughly within -1 and 1.
    #[inline]
    #[must_use]
    pub fn noise(&self, x: f64, y: f64, z: f64) -> f32 {
        let x = wrap(x) + self.x_offset;
        let y = wrap(y) + self.y_offset;
        let z = wrap(z) + self.z_offset;
        let floor_x = floor(x);
        let floor_y = floor(y);
        let floor_z = floor(z);
        let relative_y = (y - f64::from(floor_y)) as f32;
        self.sample_and_lerp(
            floor_x,
            floor_y,
            floor_z,
            (x - f64::from(floor_x)) as f32,
            relative_y,
            (z - f64::from(floor_z)) as f32,
            relative_y,
        )
    }

    /// The noise at `(x, 0, z)`, with the work along y done once when the octave was
    /// made. Noises of two dimensions, such as the surface's, are sampled so.
    #[inline]
    #[must_use]
    pub fn noise_xz(&self, x: f64, z: f64) -> f32 {
        let x = wrap(x) + self.x_offset;
        let z = wrap(z) + self.z_offset;
        let floor_x = floor(x);
        let floor_z = floor(z);
        let relative_y = self.y_offset_fraction as f32;
        self.sample_and_lerp(
            floor_x,
            self.y_offset_floor,
            floor_z,
            (x - f64::from(floor_x)) as f32,
            relative_y,
            (z - f64::from(floor_z)) as f32,
            relative_y,
        )
    }

    /// The noise at `(x, y, 0)`, with the work along z done once.
    #[inline]
    #[must_use]
    pub fn noise_xy(&self, x: f64, y: f64) -> f32 {
        let x = wrap(x) + self.x_offset;
        let y = wrap(y) + self.y_offset;
        let floor_x = floor(x);
        let floor_y = floor(y);
        let relative_y = (y - f64::from(floor_y)) as f32;
        self.sample_and_lerp(
            floor_x,
            floor_y,
            self.z_offset_floor,
            (x - f64::from(floor_x)) as f32,
            relative_y,
            self.z_offset_fraction as f32,
            relative_y,
        )
    }

    /// The noise at a point, with the height within a cell lowered by the largest
    /// multiple of `fudge_y_scale` not above it before the gradients are taken, while
    /// the easing still uses the true height (`SmearedPerlinNoise` of 26.3). The
    /// blended terrain noise is made of these; the game has always computed it so, and
    /// its terrain is what this gives.
    #[inline]
    #[must_use]
    pub fn smeared_noise(&self, x: f64, y: f64, z: f64, fudge_y_scale: f64) -> f32 {
        let original_y = y;
        let x = wrap(x) + self.x_offset;
        let y = wrap(y) + self.y_offset;
        let z = wrap(z) + self.z_offset;
        let floor_x = floor(x);
        let floor_y = floor(y);
        let floor_z = floor(z);
        let relative_y = y - f64::from(floor_y);
        let fudge_limit = if original_y >= 0.0 && original_y < relative_y {
            original_y
        } else {
            relative_y
        };
        // The game's epsilon is the float literal 1.0E-7F widened to a double.
        let fudge =
            libm::floor(fudge_limit / fudge_y_scale + f64::from(1.0e-7_f32)) * fudge_y_scale;
        self.sample_and_lerp(
            floor_x,
            floor_y,
            floor_z,
            (x - f64::from(floor_x)) as f32,
            (relative_y - fudge) as f32,
            (z - f64::from(floor_z)) as f32,
            relative_y as f32,
        )
    }

    /// The noise at a point in the older form of [`ImprovedNoise::smeared_noise`]:
    /// coordinates are not wrapped here, a `y_scale` of zero means that the height is
    /// not lowered, and `y_fudge` is the limit the smeared noise takes from the height
    /// itself. Kept for [`crate::PerlinNoise`].
    #[must_use]
    pub fn noise_with_y_scale(&self, x: f64, y: f64, z: f64, y_scale: f64, y_fudge: f64) -> f32 {
        let x = x + self.x_offset;
        let y = y + self.y_offset;
        let z = z + self.z_offset;
        let floor_x = floor(x);
        let floor_y = floor(y);
        let floor_z = floor(z);
        let relative_x = x - f64::from(floor_x);
        let relative_y = y - f64::from(floor_y);
        let relative_z = z - f64::from(floor_z);

        let fudge = if y_scale == 0.0 {
            0.0
        } else {
            let fudge_limit = if y_fudge >= 0.0 && y_fudge < relative_y {
                y_fudge
            } else {
                relative_y
            };
            libm::floor(fudge_limit / y_scale + f64::from(1.0e-7_f32)) * y_scale
        };
        self.sample_and_lerp(
            floor_x,
            floor_y,
            floor_z,
            relative_x as f32,
            (relative_y - fudge) as f32,
            relative_z as f32,
            relative_y as f32,
        )
    }

    /// The noise at a point, with its slope along each axis added to `derivative`
    /// (`PerlinNoise.noiseWithDerivative`). Added, not stored: the game sums the
    /// slopes of several octaves in one array.
    #[must_use]
    pub fn noise_with_derivative(&self, x: f64, y: f64, z: f64, derivative: &mut [f32; 3]) -> f32 {
        let x = wrap(x) + self.x_offset;
        let y = wrap(y) + self.y_offset;
        let z = wrap(z) + self.z_offset;
        let floor_x = floor(x);
        let floor_y = floor(y);
        let floor_z = floor(z);
        let rx = (x - f64::from(floor_x)) as f32;
        let ry = (y - f64::from(floor_y)) as f32;
        let rz = (z - f64::from(floor_z)) as f32;

        let x1 = floor_x.wrapping_add(1);
        let y1 = floor_y.wrapping_add(1);
        let z1 = floor_z.wrapping_add(1);
        let h000 = self.gradient_hash(floor_x, floor_y, floor_z);
        let h100 = self.gradient_hash(x1, floor_y, floor_z);
        let h010 = self.gradient_hash(floor_x, y1, floor_z);
        let h110 = self.gradient_hash(x1, y1, floor_z);
        let h001 = self.gradient_hash(floor_x, floor_y, z1);
        let h101 = self.gradient_hash(x1, floor_y, z1);
        let h011 = self.gradient_hash(floor_x, y1, z1);
        let h111 = self.gradient_hash(x1, y1, z1);

        let d000 = grad_dot(h000, rx, ry, rz);
        let d100 = grad_dot(h100, rx - 1.0, ry, rz);
        let d010 = grad_dot(h010, rx, ry - 1.0, rz);
        let d110 = grad_dot(h110, rx - 1.0, ry - 1.0, rz);
        let d001 = grad_dot(h001, rx, ry, rz - 1.0);
        let d101 = grad_dot(h101, rx - 1.0, ry, rz - 1.0);
        let d011 = grad_dot(h011, rx, ry - 1.0, rz - 1.0);
        let d111 = grad_dot(h111, rx - 1.0, ry - 1.0, rz - 1.0);

        let alpha_x = smoothstep(rx);
        let alpha_y = smoothstep(ry);
        let alpha_z = smoothstep(rz);

        // The slope has two parts. One is the gradients themselves, interpolated as
        // the values are. The other comes from the easing curve.
        let gradient_along = |axis: usize| {
            lerp3(
                alpha_x,
                alpha_y,
                alpha_z,
                GRADIENT_F32[h000 & 15][axis],
                GRADIENT_F32[h100 & 15][axis],
                GRADIENT_F32[h010 & 15][axis],
                GRADIENT_F32[h110 & 15][axis],
                GRADIENT_F32[h001 & 15][axis],
                GRADIENT_F32[h101 & 15][axis],
                GRADIENT_F32[h011 & 15][axis],
                GRADIENT_F32[h111 & 15][axis],
            )
        };
        let easing_x = lerp2(
            alpha_y,
            alpha_z,
            d100 - d000,
            d110 - d010,
            d101 - d001,
            d111 - d011,
        );
        let easing_y = lerp2(
            alpha_z,
            alpha_x,
            d010 - d000,
            d011 - d001,
            d110 - d100,
            d111 - d101,
        );
        let easing_z = lerp2(
            alpha_x,
            alpha_y,
            d001 - d000,
            d101 - d100,
            d011 - d010,
            d111 - d110,
        );
        derivative[0] += gradient_along(0) + smoothstep_derivative(rx) * easing_x;
        derivative[1] += gradient_along(1) + smoothstep_derivative(ry) * easing_y;
        derivative[2] += gradient_along(2) + smoothstep_derivative(rz) * easing_z;

        lerp3(
            alpha_x, alpha_y, alpha_z, d000, d100, d010, d110, d001, d101, d011, d111,
        )
    }

    /// The gradients at the eight corners of a cell, weighed by the position within
    /// it (`PerlinNoise.sampleAndLerp`). `original_relative_y` is the true height
    /// within the cell; the easing uses it, the gradients use `relative_y`, which the
    /// smeared noise has lowered.
    #[expect(
        clippy::too_many_arguments,
        reason = "a cell, a position within it and the unsnapped height, as in the game"
    )]
    fn sample_and_lerp(
        &self,
        x: i32,
        y: i32,
        z: i32,
        relative_x: f32,
        relative_y: f32,
        relative_z: f32,
        original_relative_y: f32,
    ) -> f32 {
        let x1 = x.wrapping_add(1);
        let y1 = y.wrapping_add(1);
        let z1 = z.wrapping_add(1);
        let relative_x1 = relative_x - 1.0;
        let relative_y1 = relative_y - 1.0;
        let relative_z1 = relative_z - 1.0;

        let d000 = self.corner(x, y, z, relative_x, relative_y, relative_z);
        let d100 = self.corner(x1, y, z, relative_x1, relative_y, relative_z);
        let d010 = self.corner(x, y1, z, relative_x, relative_y1, relative_z);
        let d110 = self.corner(x1, y1, z, relative_x1, relative_y1, relative_z);
        let d001 = self.corner(x, y, z1, relative_x, relative_y, relative_z1);
        let d101 = self.corner(x1, y, z1, relative_x1, relative_y, relative_z1);
        let d011 = self.corner(x, y1, z1, relative_x, relative_y1, relative_z1);
        let d111 = self.corner(x1, y1, z1, relative_x1, relative_y1, relative_z1);

        let x_alpha = smoothstep(relative_x);
        let y_alpha = smoothstep(original_relative_y);
        let z_alpha = smoothstep(relative_z);
        let near = lerp2(x_alpha, y_alpha, d000, d100, d010, d110);
        let far = lerp2(x_alpha, y_alpha, d001, d101, d011, d111);
        lerp(z_alpha, near, far)
    }

    /// The hash of a lattice point, whose low four bits choose its gradient. Only the
    /// low eight bits of each coordinate count, so the lattice repeats every 256.
    #[inline]
    const fn gradient_hash(&self, x: i32, y: i32, z: i32) -> usize {
        let by_x = self.permutation[(x & 0xFF) as usize];
        let by_y = self.permutation[by_x.wrapping_add((y & 0xFF) as u8) as usize];
        self.permutation[by_y.wrapping_add((z & 0xFF) as u8) as usize] as usize
    }

    #[inline]
    fn corner(&self, x: i32, y: i32, z: i32, from_x: f32, from_y: f32, from_z: f32) -> f32 {
        grad_dot(self.gradient_hash(x, y, z), from_x, from_y, from_z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random::{LegacyRandom, Xoroshiro};

    fn octave(seed: u64) -> ImprovedNoise {
        ImprovedNoise::new(&mut Xoroshiro::from_seed(seed))
    }

    #[test]
    fn the_table_is_a_permutation_and_the_offsets_are_within_a_period() {
        let noise = octave(42);
        let mut seen = [false; 256];
        for entry in noise.permutation {
            seen[usize::from(entry)] = true;
        }
        assert!(seen.iter().all(|&was_seen| was_seen));
        for offset in [noise.x_offset(), noise.y_offset(), noise.z_offset()] {
            assert!((0.0..256.0).contains(&offset));
        }
    }

    #[test]
    fn making_an_octave_draws_262_numbers_of_a_legacy_generator() {
        // This is why the game skips 262 where it leaves an octave out.
        let mut used = LegacyRandom::from_seed(99);
        let _ = ImprovedNoise::new(&mut used);
        let mut skipped = LegacyRandom::from_seed(99);
        skipped.consume_count(262);
        assert_eq!(used.state(), skipped.state());
    }

    #[test]
    fn the_same_seed_gives_the_same_octave() {
        let first = octave(12_345);
        let second = octave(12_345);
        assert_eq!(first.permutation, second.permutation);
        assert_eq!(first.x_offset().to_bits(), second.x_offset().to_bits());
        assert_eq!(
            first.noise(100.0, 64.0, 100.0).to_bits(),
            second.noise(100.0, 64.0, 100.0).to_bits()
        );
    }

    #[test]
    fn the_noise_is_zero_on_the_lattice_and_stays_within_its_range() {
        let noise = octave(42);
        let on_lattice = noise.noise(
            3.0 - noise.x_offset(),
            5.0 - noise.y_offset(),
            7.0 - noise.z_offset(),
        );
        assert!(on_lattice.abs() < 1.0e-5, "{on_lattice}");

        let mut lowest = f32::INFINITY;
        let mut highest = f32::NEG_INFINITY;
        for x in -10..10 {
            for z in -10..10 {
                let value = noise.noise(f64::from(x) * 10.3, 64.7, f64::from(z) * 10.9);
                assert!((-1.5..=1.5).contains(&value), "{value} at ({x}, {z})");
                lowest = lowest.min(value);
                highest = highest.max(value);
            }
        }
        assert!(highest - lowest > 0.5, "the noise hardly varies");
    }

    #[test]
    fn the_planes_through_zero_are_the_full_noise_there() {
        // SteelMC's cases, compared by bits.
        let noise = octave(12_345);
        let samples = [
            (0.0, 0.0),
            (1.25, -30.75),
            (-1000.0, 4096.5),
            (33_554_431.5, -33_554_432.25),
            (-0.000_000_1, 0.000_000_1),
        ];
        for (a, b) in samples {
            assert_eq!(
                noise.noise_xz(a, b).to_bits(),
                noise.noise(a, 0.0, b).to_bits(),
                "xz at ({a}, {b})"
            );
            assert_eq!(
                noise.noise_xy(a, b).to_bits(),
                noise.noise(a, b, 0.0).to_bits(),
                "xy at ({a}, {b})"
            );
        }
    }

    #[test]
    fn the_old_sampler_without_snapping_is_the_noise() {
        let noise = octave(7);
        for (x, y, z) in [(0.0, 0.0, 0.0), (1.5, 2.3, 3.7), (-5.2, 100.3, 1000.0)] {
            assert_eq!(
                noise.noise_with_y_scale(x, y, z, 0.0, 0.0).to_bits(),
                noise.noise(x, y, z).to_bits()
            );
        }
    }

    #[test]
    fn the_old_sampler_snaps_as_the_smeared_noise_does() {
        // Where no coordinate needs wrapping the two are one computation, with the
        // limit of the snapping given as the height itself.
        let noise = octave(7);
        for (x, y, z) in [(0.0, 0.0, 0.0), (1.5, 2.3, 3.7), (-5.2, 0.3, 1000.0)] {
            for scale in [0.25, 1.0, 8.0] {
                assert_eq!(
                    noise.noise_with_y_scale(x, y, z, scale, y).to_bits(),
                    noise.smeared_noise(x, y, z, scale).to_bits(),
                    "({x}, {y}, {z}), scale {scale}"
                );
            }
        }
    }

    #[test]
    fn the_smeared_noise_is_the_noise_where_nothing_is_taken_off_the_height() {
        let noise = octave(3);
        // Below zero the limit is the height within the cell, which is less than one,
        // and a step of more than one takes nothing off it.
        for (x, y, z) in [(2.5, -0.3, 4.5), (-100.25, -17.75, 3.0)] {
            assert_eq!(
                noise.smeared_noise(x, y, z, 5475.296).to_bits(),
                noise.noise(x, y, z).to_bits()
            );
        }
        // From zero up the limit is the height itself where that is less: nothing is
        // taken off at a height of zero whatever the step.
        assert_eq!(
            noise.smeared_noise(2.5, 0.0, 4.5, 0.001).to_bits(),
            noise.noise(2.5, 0.0, 4.5).to_bits()
        );
    }

    #[test]
    fn the_smeared_noise_differs_where_a_step_is_taken_off_the_height() {
        let noise = octave(3);
        // A height of 0.3 within its cell, and negative, so that the limit is 0.3.
        let y = -noise.y_offset() - 4.0 + 0.3;
        assert_ne!(
            noise.smeared_noise(2.5, y, 4.5, 0.25).to_bits(),
            noise.noise(2.5, y, 4.5).to_bits()
        );
    }

    #[test]
    fn coordinates_at_the_end_of_the_integers_do_not_overflow() {
        let mut noise = octave(42);
        noise.x_offset = 0.0;
        noise.y_offset = 0.0;
        noise.z_offset = 0.0;
        let far = f64::from(i32::MAX);
        let _ = noise.noise_with_y_scale(far, far, far, 0.0, 0.0);
        let _ = noise.noise_with_y_scale(far * 4.0, -far * 4.0, far, 0.0, 0.0);
    }

    #[test]
    fn the_value_with_slopes_is_the_noise_and_the_slopes_add_up() {
        let noise = octave(42);
        let mut slopes = [0.0_f32; 3];
        let value = noise.noise_with_derivative(1.5, 2.3, 3.7, &mut slopes);
        assert_eq!(value.to_bits(), noise.noise(1.5, 2.3, 3.7).to_bits());
        assert!(slopes.iter().any(|slope| slope.abs() > 1.0e-6));

        let first = slopes;
        let _ = noise.noise_with_derivative(4.1, 5.2, 6.3, &mut slopes);
        let mut second = [0.0_f32; 3];
        let _ = noise.noise_with_derivative(4.1, 5.2, 6.3, &mut second);
        for axis in 0..3 {
            assert_eq!(
                slopes[axis].to_bits(),
                (first[axis] + second[axis]).to_bits()
            );
        }
    }

    #[test]
    fn the_slopes_are_the_slopes_of_the_noise() {
        let noise = octave(42);
        let (x, y, z) = (1.5, 2.3, 3.7);
        let mut slopes = [0.0_f32; 3];
        let _ = noise.noise_with_derivative(x, y, z, &mut slopes);
        let step = 1.0 / 1024.0;
        let measured = [
            (noise.noise(x + step, y, z) - noise.noise(x - step, y, z)) / (2.0 * step as f32),
            (noise.noise(x, y + step, z) - noise.noise(x, y - step, z)) / (2.0 * step as f32),
            (noise.noise(x, y, z + step) - noise.noise(x, y, z - step)) / (2.0 * step as f32),
        ];
        for axis in 0..3 {
            assert!(
                (slopes[axis] - measured[axis]).abs() < 1.0e-2,
                "axis {axis}: {} against {}",
                slopes[axis],
                measured[axis]
            );
        }
    }
}
