// Adapted from SteelMC's `steel-worldgen/src/noise/simplex_noise.rs`.

//! Simplex noise, the game's `SimplexNoise`.

use crate::math::{corner_noise_3d, floor};
use crate::random::Random;

const SQRT_3: f64 = 1.732_050_807_568_877_2;
/// How far the plane is skewed to turn triangles into half squares.
const F2: f64 = 0.5 * (SQRT_3 - 1.0);
/// How far it is skewed back.
const G2: f64 = (3.0 - SQRT_3) / 6.0;
/// The same two for three dimensions.
const F3: f64 = 1.0 / 3.0;
const G3: f64 = 1.0 / 6.0;

/// One octave of simplex noise. The End's islands are made of one of these, and the
/// noises that say where a biome is cold enough for snow of several
/// ([`crate::PerlinSimplexNoise`]).
#[derive(Debug, Clone)]
pub struct SimplexNoise {
    /// The shuffled table, in the first half; the second half repeats it, as the
    /// game's does.
    permutation: [i32; 512],
    x_offset: f64,
    y_offset: f64,
    z_offset: f64,
}

impl SimplexNoise {
    /// An octave from a generator, which is advanced by three doubles and 256 bounded
    /// integers, as for [`crate::ImprovedNoise`].
    pub fn new<R: Random + ?Sized>(random: &mut R) -> Self {
        Self::with_offset_scale(random, 256.0)
    }

    /// An octave whose offsets are zero. The generator is advanced all the same.
    pub fn new_without_offset<R: Random + ?Sized>(random: &mut R) -> Self {
        Self::with_offset_scale(random, 0.0)
    }

    fn with_offset_scale<R: Random + ?Sized>(random: &mut R, offset_scale: f64) -> Self {
        let x_offset = random.next_f64() * offset_scale;
        let y_offset = random.next_f64() * offset_scale;
        let z_offset = random.next_f64() * offset_scale;

        let mut permutation = [0_i32; 512];
        for (i, entry) in permutation.iter_mut().enumerate().take(256) {
            *entry = i as i32;
        }
        for i in 0..256 {
            let offset = random.next_i32_bounded((256 - i) as i32) as usize;
            permutation.swap(i, i + offset);
        }
        permutation.copy_within(0..256, 256);

        Self {
            permutation,
            x_offset,
            y_offset,
            z_offset,
        }
    }

    /// The octave's offset along x. The samplers below do not add the offsets; the
    /// game's callers do where they want them.
    #[must_use]
    pub const fn x_offset(&self) -> f64 {
        self.x_offset
    }

    /// The octave's offset along y.
    #[must_use]
    pub const fn y_offset(&self) -> f64 {
        self.y_offset
    }

    /// The octave's offset along z.
    #[must_use]
    pub const fn z_offset(&self) -> f64 {
        self.z_offset
    }

    #[inline]
    const fn p(&self, index: i32) -> i32 {
        self.permutation[(index & 0xFF) as usize]
    }

    /// The noise at a point of the plane, roughly within -1 and 1.
    #[must_use]
    pub fn get_2d(&self, x: f64, y: f64) -> f32 {
        let skew = (x + y) * F2;
        let i = floor(x + skew);
        let j = floor(y + skew);
        let unskew = f64::from(i.wrapping_add(j)) * G2;
        let x0 = x - (f64::from(i) - unskew);
        let y0 = y - (f64::from(j) - unskew);

        // Which of the two triangles of the skewed square the point is in.
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };

        let x1 = x0 - f64::from(i1) + G2;
        let y1 = y0 - f64::from(j1) + G2;
        let x2 = x0 - 1.0 + 2.0 * G2;
        let y2 = y0 - 1.0 + 2.0 * G2;

        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let gradient0 = (self.p(ii + self.p(jj)) % 12) as usize;
        let gradient1 = (self.p(ii + i1 + self.p(jj + j1)) % 12) as usize;
        let gradient2 = (self.p(ii + 1 + self.p(jj + 1)) % 12) as usize;

        let n0 = corner_noise_3d(gradient0, x0, y0, 0.0, 0.5);
        let n1 = corner_noise_3d(gradient1, x1, y1, 0.0, 0.5);
        let n2 = corner_noise_3d(gradient2, x2, y2, 0.0, 0.5);

        (70.0 * (n0 + n1 + n2)) as f32
    }

    /// The noise at a point of space, roughly within -1 and 1.
    #[must_use]
    pub fn get_3d(&self, x: f64, y: f64, z: f64) -> f32 {
        let skew = (x + y + z) * F3;
        let i = floor(x + skew);
        let j = floor(y + skew);
        let k = floor(z + skew);
        let unskew = f64::from(i.wrapping_add(j).wrapping_add(k)) * G3;
        let x0 = x - (f64::from(i) - unskew);
        let y0 = y - (f64::from(j) - unskew);
        let z0 = z - (f64::from(k) - unskew);

        // Which of the six tetrahedra of the skewed cube the point is in: the second
        // and third corner, the first being the cube's own and the fourth opposite.
        let (i1, j1, k1, i2, j2, k2) = if x0 >= y0 {
            if y0 >= z0 {
                (1, 0, 0, 1, 1, 0)
            } else if x0 >= z0 {
                (1, 0, 0, 1, 0, 1)
            } else {
                (0, 0, 1, 1, 0, 1)
            }
        } else if y0 < z0 {
            (0, 0, 1, 0, 1, 1)
        } else if x0 < z0 {
            (0, 1, 0, 0, 1, 1)
        } else {
            (0, 1, 0, 1, 1, 0)
        };

        let x1 = x0 - f64::from(i1) + G3;
        let y1 = y0 - f64::from(j1) + G3;
        let z1 = z0 - f64::from(k1) + G3;
        let x2 = x0 - f64::from(i2) + F3;
        let y2 = y0 - f64::from(j2) + F3;
        let z2 = z0 - f64::from(k2) + F3;
        let x3 = x0 - 1.0 + 0.5;
        let y3 = y0 - 1.0 + 0.5;
        let z3 = z0 - 1.0 + 0.5;

        let ii = i & 0xFF;
        let jj = j & 0xFF;
        let kk = k & 0xFF;
        let gradient0 = (self.p(ii + self.p(jj + self.p(kk))) % 12) as usize;
        let gradient1 = (self.p(ii + i1 + self.p(jj + j1 + self.p(kk + k1))) % 12) as usize;
        let gradient2 = (self.p(ii + i2 + self.p(jj + j2 + self.p(kk + k2))) % 12) as usize;
        let gradient3 = (self.p(ii + 1 + self.p(jj + 1 + self.p(kk + 1))) % 12) as usize;

        let n0 = corner_noise_3d(gradient0, x0, y0, z0, 0.6);
        let n1 = corner_noise_3d(gradient1, x1, y1, z1, 0.6);
        let n2 = corner_noise_3d(gradient2, x2, y2, z2, 0.6);
        let n3 = corner_noise_3d(gradient3, x3, y3, z3, 0.6);

        (32.0 * (n0 + n1 + n2 + n3)) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random::LegacyRandom;

    #[test]
    fn the_same_seed_gives_the_same_octave() {
        let first = SimplexNoise::new(&mut LegacyRandom::from_seed(42));
        let second = SimplexNoise::new(&mut LegacyRandom::from_seed(42));
        for i in 0..10 {
            let x = f64::from(i) * 13.7;
            let z = f64::from(i) * 7.3;
            assert_eq!(first.get_2d(x, z).to_bits(), second.get_2d(x, z).to_bits());
            assert_eq!(
                first.get_3d(x, 0.5, z).to_bits(),
                second.get_3d(x, 0.5, z).to_bits()
            );
        }
    }

    #[test]
    fn the_second_half_of_the_table_repeats_the_first() {
        let noise = SimplexNoise::new(&mut LegacyRandom::from_seed(1));
        let mut seen = [false; 256];
        for i in 0..256 {
            assert_eq!(noise.permutation[i], noise.permutation[i + 256]);
            seen[noise.permutation[i] as usize] = true;
        }
        assert!(seen.iter().all(|&was_seen| was_seen));
    }

    #[test]
    fn an_octave_without_offset_is_the_same_octave_with_offsets_of_zero() {
        let with = SimplexNoise::new(&mut LegacyRandom::from_seed(9));
        let without = SimplexNoise::new_without_offset(&mut LegacyRandom::from_seed(9));
        assert_eq!(with.permutation, without.permutation);
        assert_eq!(without.x_offset(), 0.0);
        assert_eq!(without.y_offset(), 0.0);
        assert_eq!(without.z_offset(), 0.0);
        assert!(with.x_offset() > 0.0);
    }

    #[test]
    fn the_noise_is_zero_on_the_lattice_and_varies_within_its_range() {
        let noise = SimplexNoise::new(&mut LegacyRandom::from_seed(0));
        assert_eq!(noise.get_2d(0.0, 0.0), 0.0);
        assert_eq!(noise.get_3d(0.0, 0.0, 0.0), 0.0);

        let values: Vec<f32> = (0..200)
            .map(|i| noise.get_2d(f64::from(i) * 0.37, f64::from(i) * 0.23))
            .collect();
        let lowest = values.iter().copied().fold(f32::INFINITY, f32::min);
        let highest = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(highest - lowest > 0.5, "the noise hardly varies");
        assert!(lowest >= -1.01 && highest <= 1.01);
    }

    #[test]
    fn the_noise_of_the_ends_islands_can_be_made() {
        // The End's islands seed with zero and skip 17,292 numbers first.
        let mut random = LegacyRandom::from_seed(0);
        random.consume_count(17_292);
        let noise = SimplexNoise::new(&mut random);
        let value = noise.get_2d(10.0, 10.0);
        assert!(value.is_finite() && value.abs() > 1e-10, "{value}");
    }
}
