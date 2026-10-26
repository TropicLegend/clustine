//! The overworld's day as the game's `day` timeline drives what a server reads of it
//! (`docs/groundwork/n7d-brief.md`, sections 1.4 and 7.4): the sky's light level and the
//! darkness a bed reads, from the overworld clock's ticks, in clear weather (Clustine has
//! no other).

/// The ticks of a day (`timeline/day.json`'s `period_ticks`).
pub const DAY: u64 = 24_000;

/// The sky light's factor's keyframes: (tick of the day, factor), linear between them and
/// round the day's end.
const SKY_LIGHT: [(i64, f32); 4] = [
    (133, 1.0),
    (11_867, 1.0),
    (13_670, 0.266_666_68),
    (22_330, 0.266_666_68),
];

/// `gameplay/sky_light_level` at the overworld clock's `day` ticks: 15 times the
/// keyframed factor, in the game's float arithmetic (`KeyframeTrackSampler`,
/// `FloatModifier.MULTIPLY`).
pub fn sky_light_level(day: u64) -> f32 {
    let t = (day % DAY) as i64;
    let period = DAY as i64;
    // The keyframes before and after `t`, the last of the day before and the first of
    // the next standing in round the ends.
    let (mut from, mut to) = (SKY_LIGHT[3], SKY_LIGHT[0]);
    from.0 -= period;
    for pair in SKY_LIGHT.windows(2) {
        if pair[0].0 <= t && t < pair[1].0 {
            (from, to) = (pair[0], pair[1]);
        }
    }
    if t >= SKY_LIGHT[3].0 {
        from = SKY_LIGHT[3];
        to = (SKY_LIGHT[0].0 + period, SKY_LIGHT[0].1);
    }
    let alpha = (t - from.0) as f32 / (to.0 - from.0) as f32;
    let factor = from.1 + alpha * (to.1 - from.1);
    (15.0_f32 * factor).clamp(0.0, 15.0)
}

/// `Level.skyDarken` (`updateSkyBrightness`): how far below full the sky's light is.
pub fn sky_darken(day: u64) -> i32 {
    (15.0_f32 - sky_light_level(day)) as i32
}

/// The sun's angle in degrees at the day time `day`, as the `day` timeline's
/// `visual/sun_angle` gives it (`docs/groundwork/n8-plan.md`, section 6.4), which the
/// daylight detector reads.
///
/// The track has two keyframes at tick 6,000, 360° and then 0°, over a period of a day,
/// so `KeyframeTrackSampler` bakes the segments (−18,000, 0°) → (6,000, 360°), the empty
/// one between the two keyframes, and (6,000, 0°) → (30,000, 360°): the angle is
/// `ease(α) · 360` with α the day's share since the last noon, 0° at noon and 180° at
/// midnight. The ease and the lerp are the game's in `f32`, step for step, since the
/// detector's `round(i · cos f)` turns on a half.
pub fn sun_angle(day: u64) -> f32 {
    let t = (day % DAY) as i64;
    let period = DAY as i64;
    // The first segment whose end is after `t`.
    let from = if t < 6_000 { 6_000 - period } else { 6_000 };
    let to = from + period;
    if t <= from {
        return 0.0;
    }
    let alpha = (t - from) as f32 / (to - from) as f32;
    // `Mth.lerp(ease(alpha), 0, 360)`.
    let eased = SUN_EASE.apply(alpha);
    0.0 + eased * (360.0 - 0.0)
}

/// The visual angles' ease, `cubic_bezier` (0.362, 0.241, 0.638, 0.759) of
/// `timeline/day.json`.
const SUN_EASE: CubicBezier = CubicBezier::new(0.362, 0.241, 0.638, 0.759);

/// `EasingType.CubicBezier`: x is solved for its parameter by Newton–Raphson, and by
/// bisection where that does not come within the tolerance, and y read at it.
#[derive(Debug, Clone, Copy)]
struct CubicBezier {
    x: CubicCurve,
    y: CubicCurve,
}

/// `CubicBezier.CubicCurve`: `((a·t + b)·t + c)·t`, a curve from 0 to 1 by its two
/// control points' coordinates.
#[derive(Debug, Clone, Copy)]
struct CubicCurve {
    a: f32,
    b: f32,
    c: f32,
}

impl CubicCurve {
    /// `curveFromControls(p1, p2)`.
    const fn new(p1: f32, p2: f32) -> Self {
        Self {
            a: 3.0 * p1 - 3.0 * p2 + 1.0,
            b: -6.0 * p1 + 3.0 * p2,
            c: 3.0 * p1,
        }
    }

    fn sample(self, t: f32) -> f32 {
        ((self.a * t + self.b) * t + self.c) * t
    }

    fn sample_gradient(self, t: f32) -> f32 {
        (3.0 * self.a * t + 2.0 * self.b) * t + self.c
    }
}

impl CubicBezier {
    /// The game's tolerance and its four steps of Newton–Raphson, each at most a quarter.
    const TOLERANCE: f32 = 1.0e-5;
    const NEWTON_RAPHSON_ITERATIONS: u32 = 4;
    const MAX_STEP: f32 = 0.25;

    const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self {
            x: CubicCurve::new(x1, x2),
            y: CubicCurve::new(y1, y2),
        }
    }

    fn apply(self, x: f32) -> f32 {
        self.y.sample(self.solve_t(x))
    }

    /// `solveT`.
    fn solve_t(self, x: f32) -> f32 {
        let mut t = x;
        for _ in 0..Self::NEWTON_RAPHSON_ITERATIONS {
            let difference = self.x.sample(t) - x;
            if difference.abs() < Self::TOLERANCE {
                return t;
            }
            let gradient = self.x.sample_gradient(t);
            if gradient < Self::TOLERANCE {
                break;
            }
            t -= clamp(difference / gradient, -Self::MAX_STEP, Self::MAX_STEP);
        }
        self.solve_t_bisect(x, t)
    }

    /// `solveTBisect`, from the guess Newton–Raphson left.
    fn solve_t_bisect(self, x: f32, guess: f32) -> f32 {
        let (mut low, mut high) = (0.0_f32, 1.0_f32);
        let mut t = guess;
        while low < high {
            let difference = self.x.sample(t) - x;
            if difference.abs() < Self::TOLERANCE {
                return t;
            }
            if difference < 0.0 {
                low = t;
            } else {
                high = t;
            }
            t = (high + low) / 2.0;
        }
        t
    }
}

/// `Mth.clamp(float, float, float)`.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min { min } else { value.min(max) }
}

/// `Level.isDarkOutside`: what a bed's rule `when_dark` asks.
pub fn is_dark(day: u64) -> bool {
    sky_darken(day) >= 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bed_sleeps_from_tick_12523_to_tick_23477_of_a_clear_day() {
        let dark: Vec<u64> = (0..DAY).filter(|t| is_dark(*t)).collect();
        assert_eq!(dark.first(), Some(&12_523));
        assert_eq!(dark.last(), Some(&23_477));
        assert_eq!(dark.len() as u64, 23_477 - 12_523 + 1);
        // Every day alike.
        assert!(is_dark(5 * DAY + 12_523) && !is_dark(5 * DAY + 12_522));
    }

    #[test]
    fn the_sky_darkens_a_level_every_164_ticks_from_12031_and_is_darkest_in_the_night() {
        assert_eq!(sky_darken(12_030), 0);
        assert_eq!(sky_darken(12_031), 1);
        assert_eq!(sky_darken(12_195), 2);
        assert_eq!(sky_darken(12_359), 3);
        assert_eq!(sky_darken(12_523), 4);
        assert_eq!(sky_darken(13_670), 11);
        assert_eq!(sky_darken(22_330), 11);
        assert_eq!(sky_darken(22_331), 10);
        assert_eq!(sky_darken(23_478), 3);
        assert_eq!(sky_darken(23_970), 0);
        // The morning a sleep moves to is bright.
        assert_eq!(sky_darken(0), 0);
        assert_eq!(sky_light_level(18_000), 4.0);
    }

    /// The sun's angle at a dozen day times, as worked out apart from this code from the
    /// dump's arithmetic in single precision (`EasingType$CubicBezier`, the keyframes of
    /// `timeline/day.json`), bit for bit: 0° at noon, 180° at midnight, a whole turn by
    /// the next noon, and every day alike.
    #[test]
    fn the_suns_angle_is_the_eased_share_of_the_day_since_noon() {
        for (day, bits, about) in [
            (0, 0x438d_2fea, 282.374_33),
            (1_000, 0x4394_c5da, 297.545_7),
            (2_000, 0x439b_f4df, 311.913_05),
            (5_999, 0x43b3_fe9d, 359.989_17),
            (6_000, 0x0000_0000, 0.0),
            (6_001, 0x3c31_b168, 0.010_845_52),
            (9_000, 0x420a_64ee, 34.598_564),
            (12_000, 0x429b_4057, 77.625_66),
            (13_000, 0x42bb_0aec, 93.521_33),
            (18_000, 0x4334_0000, 180.0),
            (22_000, 0x4379_f61b, 249.961_35),
            (23_999, 0x438d_2ded, 282.358_8),
            (24_000, 0x438d_2fea, 282.374_33),
            (30_000, 0x0000_0000, 0.0),
        ] {
            let angle = sun_angle(day);
            assert_eq!(angle.to_bits(), bits, "at {day}: {angle}");
            assert_eq!(angle, f32::from_bits(bits));
            assert!((angle - about).abs() < 1.0e-3, "at {day}: {angle}");
        }
    }
}
