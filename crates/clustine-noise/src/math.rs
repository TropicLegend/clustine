// Adapted from the scalar halves of SteelMC's `steel-math/src/noise_math/`
// (`coordinate.rs`, `gradient.rs`, `interpolation.rs` and `mod.rs`).

//! The arithmetic of the game's `Mth` and `NoiseUtils` that the noises are built from.
//!
//! The order of the operations in each function is the game's. Changing `a + t * (b -
//! a)` into another form of the same formula changes the last bit of some results, and
//! a block of some chunk with it.

use std::f64::consts::PI;
use std::ops::{Add, Div, Mul, Sub};

pub use crate::ln::ln;

/// A number the interpolations are written for: `f32` and `f64`. The game has each of
/// them once per width, with the same formula.
pub trait Real:
    Copy
    + PartialOrd
    + From<f32>
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
{
}

impl Real for f32 {}
impl Real for f64 {}

/// The largest integer not above `v` (`Mth.floor`): the cell of the lattice a
/// coordinate lies in. Beyond the range of `i32` it sticks at the ends as Java's cast
/// does, and wraps where Java's subtraction wraps.
#[inline]
#[must_use]
pub fn floor(v: f64) -> i32 {
    let truncated = v as i32;
    if v < f64::from(truncated) {
        truncated.wrapping_sub(1)
    } else {
        truncated
    }
}

/// [`floor`] to an `i64` (`Mth.lfloor`).
#[inline]
#[must_use]
pub fn lfloor(v: f64) -> i64 {
    let truncated = v as i64;
    if v < truncated as f64 {
        truncated.wrapping_sub(1)
    } else {
        truncated
    }
}

/// The period of [`wrap`], 2^25.
const ROUND_OFF: f64 = 33_554_432.0;
const HALF_ROUND_OFF: f64 = ROUND_OFF / 2.0;

/// Brings a coordinate of a noise into `[-2^24, 2^24)` (`PerlinNoise.wrap`), so that a
/// noise keeps its precision far from the origin. The noise repeats with that period.
#[inline]
#[must_use]
pub fn wrap(x: f64) -> f64 {
    // Within the range the formula below subtracts zero, so this only saves time.
    if (-HALF_ROUND_OFF..HALF_ROUND_OFF).contains(&x) {
        return x;
    }
    x - libm::floor(x / ROUND_OFF + 0.5) * ROUND_OFF
}

/// `value` held within `min` and `max` (`Mth.clamp`).
#[inline]
#[must_use]
pub fn clamp<F: Real>(value: F, min: F, max: F) -> F {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// [`clamp`] for integers.
#[inline]
#[must_use]
pub const fn clamp_i32(value: i32, min: i32, max: i32) -> i32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// The point a share `alpha` of the way from `a` to `b` (`Mth.lerp`).
#[inline]
#[must_use]
pub fn lerp<F: Real>(alpha: F, a: F, b: F) -> F {
    a + alpha * (b - a)
}

/// [`lerp`] over a square: along the first axis by `a1`, then along the second by `a2`
/// (`Mth.lerp2`).
#[inline]
#[must_use]
pub fn lerp2<F: Real>(a1: F, a2: F, x00: F, x10: F, x01: F, x11: F) -> F {
    lerp(a2, lerp(a1, x00, x10), lerp(a1, x01, x11))
}

/// [`lerp`] over a cube (`Mth.lerp3`). The noise chunk fills the blocks of a cell from
/// the densities at its eight corners with this.
#[inline]
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the eight corners of a cube and a share for each axis, as in the game"
)]
pub fn lerp3<F: Real>(
    a1: F,
    a2: F,
    a3: F,
    x000: F,
    x100: F,
    x010: F,
    x110: F,
    x001: F,
    x101: F,
    x011: F,
    x111: F,
) -> F {
    lerp(
        a3,
        lerp2(a1, a2, x000, x100, x010, x110),
        lerp2(a1, a2, x001, x101, x011, x111),
    )
}

/// [`lerp`] that stops at its ends (`Mth.clampedLerp`). The game's order of arguments
/// is the share first; this one has it last, as SteelMC has.
#[inline]
#[must_use]
pub fn clamped_lerp<F: Real>(min: F, max: F, factor: F) -> F {
    if factor < F::from(0.0) {
        min
    } else if factor > F::from(1.0) {
        max
    } else {
        lerp(factor, min, max)
    }
}

/// The share of the way from `a` to `b` at which `value` lies (`Mth.inverseLerp`).
#[inline]
#[must_use]
pub fn inverse_lerp<F: Real>(value: F, a: F, b: F) -> F {
    (value - a) / (b - a)
}

/// `value` carried from one range to another, beyond their ends too (`Mth.map`).
#[inline]
#[must_use]
pub fn map<F: Real>(value: F, from_min: F, from_max: F, to_min: F, to_max: F) -> F {
    lerp(inverse_lerp(value, from_min, from_max), to_min, to_max)
}

/// [`map`] that stops at the ends of the target range (`Mth.clampedMap`). The density
/// functions that depend on height alone are this.
#[inline]
#[must_use]
pub fn map_clamped<F: Real>(value: F, from_min: F, from_max: F, to_min: F, to_max: F) -> F {
    clamped_lerp(to_min, to_max, inverse_lerp(value, from_min, from_max))
}

/// The curve `6x^5 - 15x^4 + 10x^3` (`Mth.smoothstep`), with which Perlin noise eases
/// from one lattice point to the next.
#[inline]
#[must_use]
pub fn smoothstep<F: Real>(x: F) -> F {
    x * x * x * (x * (x * F::from(6.0) - F::from(15.0)) + F::from(10.0))
}

/// The slope of [`smoothstep`], `30x^2(x - 1)^2` (`Mth.smoothstepDerivative`).
#[inline]
#[must_use]
pub fn smoothstep_derivative<F: Real>(x: F) -> F {
    F::from(30.0) * x * x * (x - F::from(1.0)) * (x - F::from(1.0))
}

/// Pushes a noise value towards -1 and 1 (`NoiseUtils.biasTowardsExtreme`).
///
/// The sine is `libm`'s, which is the one of Java's `StrictMath`.
#[inline]
#[must_use]
pub fn bias_towards_extreme(noise: f64, factor: f64) -> f64 {
    noise + libm::sin(PI * noise) * factor / PI
}

/// `x` cubed (`Mth.cube`).
#[inline]
#[must_use]
pub fn cube(x: f64) -> f64 {
    x * x * x
}

/// `x` squared (`Mth.square`).
#[inline]
#[must_use]
pub fn square(x: f64) -> f64 {
    x * x
}

/// Two to the power of `exponent`, exactly. The frequencies and amplitudes of octaves
/// are such powers; the game takes them from `Math.pow`, which is exact for them too.
#[inline]
#[must_use]
pub fn pow2(exponent: i32) -> f64 {
    libm::scalbn(1.0, exponent)
}

/// The sixteen gradients Perlin and simplex noise choose from (`SimplexNoise.GRADIENT`).
/// The last four repeat earlier ones, so that the choice can be four bits of a hash.
pub const GRADIENT: [[f64; 3]; 16] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 1.0, 0.0],
    [0.0, -1.0, 1.0],
    [-1.0, 1.0, 0.0],
    [0.0, -1.0, -1.0],
];

/// [`GRADIENT`] in single precision, in which 26.3 computes Perlin noise.
pub const GRADIENT_F32: [[f32; 3]; 16] = gradient_f32();

const fn gradient_f32() -> [[f32; 3]; 16] {
    let mut result = [[0.0; 3]; 16];
    let mut i = 0;
    while i < GRADIENT.len() {
        result[i] = [
            GRADIENT[i][0] as f32,
            GRADIENT[i][1] as f32,
            GRADIENT[i][2] as f32,
        ];
        i += 1;
    }
    result
}

/// The product of a gradient and an offset from a lattice point, in double precision
/// (`SimplexNoise.dot`).
#[inline]
#[must_use]
pub fn dot(gradient: &[f64; 3], x: f64, y: f64, z: f64) -> f64 {
    gradient[0] * x + gradient[1] * y + gradient[2] * z
}

/// The product of the gradient a hash chooses and an offset from a lattice point, in
/// single precision. All three terms are added, in this order, although one of them is
/// always zero: a zero term can still change the sign of a zero sum.
#[inline]
#[must_use]
pub fn grad_dot(hash: usize, x: f32, y: f32, z: f32) -> f32 {
    let gradient = &GRADIENT_F32[hash & 15];
    gradient[0] * x + gradient[1] * y + gradient[2] * z
}

/// What one corner of a simplex adds to simplex noise (`SimplexNoise.getCornerNoise3D`).
/// `base` is 0.5 in two dimensions and 0.6 in three.
#[inline]
#[must_use]
pub fn corner_noise_3d(index: usize, x: f64, y: f64, z: f64, base: f64) -> f64 {
    let falloff = base - x * x - y * y - z * z;
    if falloff < 0.0 {
        0.0
    } else {
        let falloff = falloff * falloff;
        falloff * falloff * dot(&GRADIENT[index], x, y, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_rounds_down_on_both_sides_of_zero() {
        // SteelMC's cases.
        assert_eq!(floor(1.5), 1);
        assert_eq!(floor(1.0), 1);
        assert_eq!(floor(0.5), 0);
        assert_eq!(floor(0.0), 0);
        assert_eq!(floor(-0.5), -1);
        assert_eq!(floor(-1.0), -1);
        assert_eq!(floor(-1.5), -2);
        assert_eq!(lfloor(-1.5), -2);
        assert_eq!(lfloor(3_000_000_000.5), 3_000_000_000);
    }

    #[test]
    fn floor_beyond_the_integers_does_what_javas_does() {
        // (int) sticks at the ends, and Java's `i - 1` then wraps.
        assert_eq!(floor(1.0e10), i32::MAX);
        assert_eq!(floor(-1.0e10), i32::MAX);
        assert_eq!(floor(f64::NAN), 0);
    }

    #[test]
    fn wrap_leaves_near_coordinates_and_folds_far_ones() {
        fn by_the_formula(x: f64) -> f64 {
            x - libm::floor(x / ROUND_OFF + 0.5) * ROUND_OFF
        }
        assert_eq!(wrap(100.0).to_bits(), 100.0_f64.to_bits());
        assert_eq!(wrap(-100.0).to_bits(), (-100.0_f64).to_bits());
        assert_eq!(wrap(-0.0).to_bits(), (-0.0_f64).to_bits());
        assert!(wrap(100_000_000.0).abs() < ROUND_OFF);
        for x in [
            -HALF_ROUND_OFF,
            -HALF_ROUND_OFF + 1.0,
            -0.25,
            0.0,
            HALF_ROUND_OFF - 1.0,
            HALF_ROUND_OFF,
            ROUND_OFF,
            -ROUND_OFF,
            100_000_000.0,
            -100_000_000.0,
            1.0e300,
        ] {
            assert_eq!(wrap(x).to_bits(), by_the_formula(x).to_bits(), "{x}");
        }
    }

    #[test]
    fn interpolations_meet_their_ends() {
        assert_eq!(lerp(0.0_f64, 10.0, 20.0), 10.0);
        assert_eq!(lerp(1.0_f64, 10.0, 20.0), 20.0);
        assert_eq!(lerp(0.5_f32, 10.0, 20.0), 15.0);
        assert_eq!(lerp2(0.5_f64, 0.5, 0.0, 2.0, 4.0, 6.0), 3.0);
        assert_eq!(
            lerp3(1.0_f32, 1.0, 1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0),
            7.0
        );
        assert_eq!(clamped_lerp(1.0_f32, 3.0, -0.5), 1.0);
        assert_eq!(clamped_lerp(1.0_f32, 3.0, 1.5), 3.0);
        assert_eq!(clamped_lerp(1.0_f64, 3.0, 0.5), 2.0);
        assert_eq!(inverse_lerp(15.0_f64, 10.0, 20.0), 0.5);
        assert_eq!(map(30.0_f64, 10.0, 20.0, 0.0, 1.0), 2.0);
        assert_eq!(map_clamped(30.0_f64, 10.0, 20.0, 0.0, 1.0), 1.0);
        assert_eq!(map_clamped(15.0_f32, 10.0, 20.0, 0.0, 1.0), 0.5);
        assert_eq!(clamp(5.0_f64, 0.0, 1.0), 1.0);
        assert_eq!(clamp(-5.0_f32, 0.0, 1.0), 0.0);
        assert_eq!(clamp_i32(5, 0, 10), 5);
    }

    #[test]
    fn smoothstep_runs_from_zero_to_one_and_is_flat_at_both() {
        assert_eq!(smoothstep(0.0_f64), 0.0);
        assert_eq!(smoothstep(1.0_f64), 1.0);
        assert_eq!(smoothstep(0.5_f64), 0.5);
        assert_eq!(smoothstep(0.5_f32), 0.5);
        assert_eq!(smoothstep_derivative(0.0_f64), 0.0);
        assert_eq!(smoothstep_derivative(1.0_f32), 0.0);
        assert_eq!(smoothstep_derivative(0.5_f64), 1.875);
    }

    #[test]
    fn powers_of_two_are_exact() {
        assert_eq!(pow2(0), 1.0);
        assert_eq!(pow2(10), 1024.0);
        assert_eq!(pow2(-7), 0.007_812_5);
        assert_eq!(pow2(-15).to_bits(), (1.0_f64 / 32_768.0).to_bits());
    }

    #[test]
    fn the_single_precision_gradients_are_the_double_ones() {
        for (single, double) in GRADIENT_F32.iter().zip(&GRADIENT) {
            for axis in 0..3 {
                assert_eq!(f64::from(single[axis]), double[axis]);
            }
        }
        assert_eq!(grad_dot(16 + 5, 2.0, 3.0, 5.0), -2.0 + 5.0);
        assert_eq!(dot(&GRADIENT[5], 2.0, 3.0, 5.0), 3.0);
    }

    #[test]
    fn a_far_corner_of_a_simplex_adds_nothing() {
        assert_eq!(corner_noise_3d(0, 1.0, 0.0, 0.0, 0.5), 0.0);
        // 0.5 - 0.25 = 0.25; to the fourth power, times the gradient (1, 1, 0) along x.
        assert_eq!(corner_noise_3d(0, 0.5, 0.0, 0.0, 0.5), 0.003_906_25 * 0.5);
    }

    #[test]
    fn the_bias_leaves_the_ends_and_the_middle_nearly_where_they_are() {
        assert_eq!(bias_towards_extreme(0.0, 0.3), 0.0);
        assert!((bias_towards_extreme(1.0, 0.3) - 1.0).abs() < 1e-15);
        assert!(bias_towards_extreme(0.5, 0.3) > 0.5);
    }
}
