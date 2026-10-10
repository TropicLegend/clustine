// Adapted from SteelMC's `steel-math/src/angle.rs`.

//! Angles in degrees, as the game keeps the rotations of entities, signs and pieces of
//! structures.

use std::f32::consts::PI as PI_F32;
use std::f64::consts::PI as PI_F64;

/// A quarter turn in degrees.
pub const DEGREE_90: f32 = 90.0;
/// A half turn in degrees.
pub const DEGREE_180: f32 = 180.0;
/// Three quarters of a turn in degrees.
pub const DEGREE_270: f32 = 270.0;
/// A full turn in degrees.
pub const DEGREE_360: f32 = 360.0;

/// Radians per degree in single precision (`Mth.DEG_TO_RAD`).
pub const DEG_TO_RAD: f32 = PI_F32 / DEGREE_180;
/// Degrees per radian in single precision (`Mth.RAD_TO_DEG`).
pub const RAD_TO_DEG: f32 = DEGREE_180 / PI_F32;
/// Radians per degree in double precision.
pub const DEG_TO_RAD_F64: f64 = PI_F64 / DEGREE_180 as f64;
/// Degrees per radian in double precision.
pub const RAD_TO_DEG_F64: f64 = DEGREE_180 as f64 / PI_F64;

/// An angle brought into `[-180, 180)` (`Mth.wrapDegrees`).
#[must_use]
pub fn wrap_degrees(degrees: f32) -> f32 {
    // The remainder of floats is exact and has the sign of the dividend, as Java's.
    let mut degrees = degrees % DEGREE_360;
    if degrees >= DEGREE_180 {
        degrees -= DEGREE_360;
    }
    if degrees < -DEGREE_180 {
        degrees += DEGREE_360;
    }
    degrees
}

/// The sixteenth of a turn nearest to an angle, as standing signs, banners and heads
/// store their rotation (`RotationSegment.convertToSegment`).
#[must_use]
pub fn convert_to_rotation_segment(degrees: f32) -> u8 {
    (((degrees.rem_euclid(DEGREE_360) / (DEGREE_360 / 16.0)) + 0.5) as u8) & 15
}

#[cfg(test)]
mod tests {
    // SteelMC's cases.

    use super::*;

    #[test]
    fn angles_wrap_into_the_half_open_turn() {
        assert_eq!(wrap_degrees(181.0).to_bits(), (-179.0_f32).to_bits());
        assert_eq!(wrap_degrees(-181.0).to_bits(), 179.0_f32.to_bits());
        assert_eq!(wrap_degrees(90.0).to_bits(), 90.0_f32.to_bits());
        assert_eq!(wrap_degrees(540.0).to_bits(), (-180.0_f32).to_bits());
    }

    #[test]
    fn rotation_segments_round_to_the_nearest_and_wrap() {
        assert_eq!(convert_to_rotation_segment(11.24), 0);
        assert_eq!(convert_to_rotation_segment(11.25), 1);
        assert_eq!(convert_to_rotation_segment(-90.0), 12);
        assert_eq!(convert_to_rotation_segment(360.0), 0);
    }
}
