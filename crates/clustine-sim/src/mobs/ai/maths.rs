//! The game's arithmetic of angles and delays that the AI uses (`Mth`, `Control`, the
//! brief's 2.3), in `f32` where the game uses `float`, and the HotSpot sine and cosine
//! of two goals (`Math.sin`, `Math.cos`, which the plan's decision 7 takes as
//! `StrictMath`'s, here `libm`'s).

/// The factor from radians to degrees the controls multiply by (`57.2957763671875`, the
/// float `57.295776f` widened, as the bytecode has it).
pub(crate) const DEGREES: f64 = 57.295_776_367_187_5;

/// The float factor from degrees to radians (`0.017453292f`).
pub(crate) const RADIANS: f32 = 0.017_453_292;

/// `Mth.clamp(float, float, float)`: below the least is the least, else the lesser of the
/// value and the most (so a value that is not a number comes back as the most).
pub(crate) fn clamp(value: f32, least: f32, most: f32) -> f32 {
    if value < least {
        least
    } else {
        // `Math.min(value, most)`.
        if value.is_nan() || most.is_nan() {
            f32::NAN
        } else if value <= most {
            value
        } else {
            most
        }
    }
}

/// `Mth.clamp(double, double, double)`.
pub(crate) fn clamp_f64(value: f64, least: f64, most: f64) -> f64 {
    if value < least {
        least
    } else if value.is_nan() || most.is_nan() {
        f64::NAN
    } else if value <= most {
        value
    } else {
        most
    }
}

/// `Mth.wrapDegrees(float)`: into [-180, 180), the remainder taking the dividend's
/// sign as Java's does.
pub(crate) fn wrap_degrees(degrees: f32) -> f32 {
    let mut wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// `Control.rotateTowards(from, to, max)` (and `Mob.rotlerp`, the same sum): `from`
/// turned towards `to` by at most `max`.
pub(crate) fn rotate_towards(from: f32, to: f32, max: f32) -> f32 {
    from + clamp(wrap_degrees(to - from), -max, max)
}

/// `Mth.rotateIfNecessary(a, b, max)`: `a` brought within `max` of `b`.
pub(crate) fn rotate_if_necessary(a: f32, b: f32, max: f32) -> f32 {
    b - clamp(wrap_degrees(b - a), -max, max)
}

/// `MoveControl.rotlerp(from, to, max)`: as [`rotate_towards`], then kept in [0, 360]
/// (the brief's 2.0.13).
pub(crate) fn move_rotlerp(from: f32, to: f32, max: f32) -> f32 {
    let turned = from + clamp(wrap_degrees(to - from), -max, max);
    if turned < 0.0 {
        turned + 360.0
    } else if turned > 360.0 {
        turned - 360.0
    } else {
        turned
    }
}

/// `Mth.atan2`, the game's own from tables.
pub(crate) fn atan2(y: f64, x: f64) -> f64 {
    crate::physics::minecart::atan2(y, x)
}

/// The yaw that faces along `(dx, dz)`, as the controls compute it: `(float)(Mth.atan2(dz,
/// dx) * 57.2957763671875) - 90f`.
pub(crate) fn yaw_towards(dx: f64, dz: f64) -> f32 {
    (atan2(dz, dx) * DEGREES) as f32 - 90.0
}

/// The pitch that looks up `dy` over `horizontal`: `(float)(-(Mth.atan2(dy, h) *
/// 57.2957763671875))`, negated in double and then narrowed.
pub(crate) fn pitch_towards(dy: f64, horizontal: f64) -> f32 {
    (-(atan2(dy, horizontal) * DEGREES)) as f32
}

/// `Mth.sin(float)` of an angle in radians: the game's table.
pub(crate) fn table_sin(radians: f32) -> f32 {
    clustine_noise::trig::sin(f64::from(radians))
}

/// `Mth.cos(float)`.
pub(crate) fn table_cos(radians: f32) -> f32 {
    clustine_noise::trig::cos(f64::from(radians))
}

/// `Math.sin(double)`, HotSpot's, taken as `StrictMath`'s (a named difference).
pub(crate) fn hotspot_sin(radians: f64) -> f64 {
    libm::sin(radians)
}

/// `Math.cos(double)`, as [`hotspot_sin`].
pub(crate) fn hotspot_cos(radians: f64) -> f64 {
    libm::cos(radians)
}

/// `Mth.positiveCeilDiv(n, 2)`, which is `reducedTickDelay(n)`: the ceiling of half.
pub(crate) fn reduced_tick_delay(ticks: i32) -> i32 {
    -(-ticks).div_euclid(2)
}

/// `Mth.lerp(double t, double a, double b)`.
pub(crate) fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

/// `Mth.sqrt(float)`: through `double`, which rounds as `f32::sqrt` does.
pub(crate) fn sqrt_f32(value: f32) -> f32 {
    (f64::from(value)).sqrt() as f32
}

/// `Mth.floor(float)`.
pub(crate) fn floor_f32(value: f32) -> i32 {
    value.floor() as i32
}

/// `Mth.ceil(double)`.
pub(crate) fn ceil(value: f64) -> i32 {
    value.ceil() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_degrees_keep_the_dividend_s_sign_before_they_are_folded() {
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(-180.0), -180.0);
        assert_eq!(wrap_degrees(180.0), -180.0);
        assert_eq!(wrap_degrees(540.0), -180.0);
    }

    #[test]
    fn the_move_control_s_turn_stays_between_0_and_360() {
        assert_eq!(move_rotlerp(10.0, -30.0, 90.0), 330.0);
        assert_eq!(move_rotlerp(350.0, 30.0, 90.0), 30.0);
        assert_eq!(rotate_towards(10.0, -30.0, 90.0), -30.0);
    }

    #[test]
    fn a_reduced_delay_is_the_ceiling_of_half() {
        assert_eq!(reduced_tick_delay(120), 60);
        assert_eq!(reduced_tick_delay(5), 3);
        assert_eq!(reduced_tick_delay(0), 0);
        assert_eq!(reduced_tick_delay(1), 1);
    }
}
