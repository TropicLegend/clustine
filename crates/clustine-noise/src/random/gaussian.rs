// Adapted from SteelMC's `steel-utils/src/random/gaussian.rs`.

//! Normally distributed numbers by Marsaglia's polar method, as the game's
//! `MarsagliaPolarGaussian`.

use super::Random;
use crate::math::ln;

/// The next normally distributed number of a generator.
///
/// The method makes two numbers at a time. The second waits in `stored` for the next
/// call, which is why every generator carries such a slot.
///
/// The logarithm is this crate's correctly rounded one, not `libm`'s: see
/// [`crate::math::ln`] for why. The square root is `libm`'s, which is exact to the
/// last bit as every square root is.
pub(super) fn next_gaussian<R: Random + ?Sized>(random: &mut R, stored: &mut Option<f64>) -> f64 {
    if let Some(gaussian) = stored.take() {
        return gaussian;
    }
    loop {
        let x = 2.0 * random.next_f64() - 1.0;
        let y = 2.0 * random.next_f64() - 1.0;
        let radius_squared = x * x + y * y;
        if radius_squared < 1.0 && radius_squared != 0.0 {
            let factor = libm::sqrt(-2.0 * ln(radius_squared) / radius_squared);
            *stored = Some(y * factor);
            return x * factor;
        }
    }
}
