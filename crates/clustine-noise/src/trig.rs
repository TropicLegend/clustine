// Adapted from SteelMC's `steel-math/src/trig.rs`.

//! The game's table of sines (`Mth.sin` and `Mth.cos`).
//!
//! Carvers, ore veins, dripstone and much of the game outside world generation take
//! their sines from a table of 65,536 floats, not from the mathematics library. A
//! cave bends where the game bends it only with the same table, looked up the same way.

use std::sync::LazyLock;

/// Entries per radian: 65,536 over a full turn, as the game writes it.
const INDEX_SCALE: f64 = 10_430.378_350_470_453;
const TABLE_LENGTH: usize = 65_536;
const TABLE_MASK: i64 = 0xFFFF;

/// The table. The game fills it with `Math.sin`; this fills it with `libm`'s sine,
/// which is the one of Java's `StrictMath`, so that every platform has the same table.
static SIN_TABLE: LazyLock<Box<[f32]>> = LazyLock::new(|| {
    (0..TABLE_LENGTH)
        .map(|i| libm::sin(i as f64 / INDEX_SCALE) as f32)
        .collect()
});

/// One entry of the table, for comparing it with the game's.
#[must_use]
pub fn sin_table_entry(index: u16) -> f32 {
    SIN_TABLE[usize::from(index)]
}

/// The sine of an angle in radians, to the table's step (`Mth.sin`).
#[inline]
#[must_use]
pub fn sin(angle: f64) -> f32 {
    let index = (((angle * INDEX_SCALE) as i64) & TABLE_MASK) as usize;
    SIN_TABLE[index]
}

/// The cosine of an angle in radians: the same table a quarter turn on (`Mth.cos`).
#[inline]
#[must_use]
pub fn cos(angle: f64) -> f32 {
    let index = (((angle * INDEX_SCALE + 16_384.0) as i64) & TABLE_MASK) as usize;
    SIN_TABLE[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_holds_the_quarter_turns_exactly() {
        assert_eq!(sin(0.0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(cos(0.0).to_bits(), 1.0_f32.to_bits());
        assert_eq!(sin_table_entry(16_384).to_bits(), 1.0_f32.to_bits());
        assert_eq!(sin_table_entry(49_152).to_bits(), (-1.0_f32).to_bits());
    }

    #[test]
    fn the_table_is_the_rounded_sine_of_each_step() {
        for index in [0_u16, 1, 1000, 16_384, 32_768, 49_152, 65_535] {
            let expected = libm::sin(f64::from(index) / INDEX_SCALE) as f32;
            assert_eq!(sin_table_entry(index).to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn angles_are_cut_to_a_step_and_wrap_around_the_circle() {
        let step = 1.0 / INDEX_SCALE;
        assert_eq!(sin(0.7).to_bits(), sin_table_entry(7301).to_bits());
        assert_eq!(cos(0.7).to_bits(), sin_table_entry(7301 + 16_384).to_bits());
        // A negative angle is cut towards zero before the low bits are taken.
        assert_eq!(sin(-0.5 * step).to_bits(), sin_table_entry(0).to_bits());
        assert_eq!(
            sin(-1.5 * step).to_bits(),
            sin_table_entry(65_535).to_bits()
        );
        assert_eq!(sin(65_536.5 * step).to_bits(), sin_table_entry(0).to_bits());
    }
}
