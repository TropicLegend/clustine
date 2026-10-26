//! The daylight detector (n8 DaylightDetectorBlock 42–161, EasingType$CubicBezier 5–160,
//! `timeline/day.json` as the plan's 6.4 reads it), against a port of the game's sums
//! written here from the dumps: the sun's angle eased in `f32`, the power through a whole
//! day and at every tick where `round(i · cos f)` turns, inverted, gated on the
//! twentieth tick, and clicked.

use super::*;
use crate::time::{sky_darken, sun_angle};

/// `EasingType$CubicBezier$CubicCurve`: `((a·t + b)·t + c)·t`.
#[derive(Clone, Copy)]
struct Curve {
    a: f32,
    b: f32,
    c: f32,
}

impl Curve {
    /// `curveFromControls(p1, p2)`.
    fn of(p1: f32, p2: f32) -> Self {
        Self {
            a: 3.0 * p1 - 3.0 * p2 + 1.0,
            b: -6.0 * p1 + 3.0 * p2,
            c: 3.0 * p1,
        }
    }

    fn sample(self, t: f32) -> f32 {
        ((self.a * t + self.b) * t + self.c) * t
    }

    fn gradient(self, t: f32) -> f32 {
        (3.0 * self.a * t + 2.0 * self.b) * t + self.c
    }
}

/// The sun's ease, `cubic_bezier` (0.362, 0.241, 0.638, 0.759), step for step.
fn ease(x: f32) -> f32 {
    let along = Curve::of(0.362, 0.638);
    let up = Curve::of(0.241, 0.759);
    let mut t = x;
    let mut solved = None;
    for _ in 0..4 {
        let error = along.sample(t) - x;
        if error.abs() < 1.0e-5 {
            solved = Some(t);
            break;
        }
        let gradient = along.gradient(t);
        if gradient < 1.0e-5 {
            break;
        }
        t -= (error / gradient).clamp(-0.25, 0.25);
    }
    let t = solved.unwrap_or_else(|| {
        // `solveTBisect`, from where Newton's steps stopped.
        let (mut low, mut high) = (0.0_f32, 1.0_f32);
        let mut t = t;
        while low < high {
            let error = along.sample(t) - x;
            if error.abs() < 1.0e-5 {
                return t;
            }
            if error < 0.0 {
                low = t;
            } else {
                high = t;
            }
            t = (high + low) / 2.0;
        }
        t
    });
    up.sample(t)
}

/// The sun's angle in degrees at `day`, by the plan's 6.4: `ease(α) · 360` with α the
/// day's share since the last noon.
fn angle(day: u64) -> f32 {
    let alpha = (day as i64 - 6_000).rem_euclid(24_000) as f32 / 24_000.0;
    0.0 + ease(alpha) * (360.0 - 0.0)
}

/// `Math.round(float)`: the nearest whole number, a half rounding up.
fn round(value: f32) -> i32 {
    (f64::from(value) + 0.5).floor() as i32
}

/// `updateSignalStrength`'s power for a detector under a sky of `sky` at `day`.
fn power(sky: i32, day: u64, inverted: bool) -> i32 {
    let mut light = (sky - sky_darken(day)).max(0);
    let mut f = angle(day) * 0.017_453_292;
    if inverted {
        light = 15 - light;
    } else if light > 0 {
        let towards = if f < std::f32::consts::PI {
            0.0
        } else {
            std::f32::consts::TAU
        };
        f += (towards - f) * 0.2;
        light = round(light as f32 * clustine_noise::trig::cos(f64::from(f)));
    }
    light.clamp(0, 15)
}

fn detector(inverted: bool, power: i32) -> BlockState {
    state(&format!(
        "minecraft:daylight_detector[inverted={inverted},power={power}]"
    ))
}

fn power_of(state: BlockState) -> i32 {
    state
        .properties()
        .find(|(name, _)| *name == "power")
        .and_then(|(_, value)| value.parse().ok())
        .expect("a detector has a power")
}

#[test]
fn the_sun_s_angle_is_the_eased_share_of_the_day_bit_for_bit() {
    for day in 0..24_000 {
        let expected = angle(day);
        let made = sun_angle(day);
        assert!(
            made.to_bits() == expected.to_bits(),
            "day {day}: {made} where the game's sums give {expected}"
        );
    }
    assert_eq!(sun_angle(6_000), 0.0, "noon");
    assert!((sun_angle(18_000) - 180.0).abs() < 1.0e-3, "midnight");
    // A day later, the same.
    assert_eq!(sun_angle(30_000).to_bits(), sun_angle(6_000).to_bits());
}

/// The days of one day on which a detector in the open, or an inverted one, gives other
/// power than the day before: where the sky darkens a step or `round(i · cos f)` turns.
fn turns(inverted: bool) -> Vec<u64> {
    (1..24_000)
        .filter(|day| power(15, *day, inverted) != power(15, day - 1, inverted))
        .collect()
}

#[test]
fn a_detector_in_the_open_gives_the_game_s_power_on_either_side_of_every_turn() {
    let plain = at(0, GROUND, 0);
    let inverted = at(2, GROUND, 0);
    let mut region = open(&[(plain, detector(false, 0)), (inverted, detector(true, 0))]);
    assert_eq!(levels(&region, plain).map(|(sky, _)| sky), Some(15));
    assert_eq!(levels(&region, inverted).map(|(sky, _)| sky), Some(15));
    region.clock.game_time = 40;
    let mut days: Vec<u64> = (0..24_000).step_by(97).collect();
    for kind in [false, true] {
        for day in turns(kind) {
            days.extend([day - 1, day]);
        }
    }
    assert!(
        turns(false).len() > 20,
        "the power rises and falls by steps"
    );
    days.extend([0, 6_000, 12_000, 13_000, 18_000, 23_000, 23_999]);
    days.sort_unstable();
    days.dedup();
    for day in days {
        region.clock.day = day;
        let mut output = TickOutput::default();
        for (pos, kind) in [(plain, false), (inverted, true)] {
            let now = read(&region, pos);
            region.daylight_detector_tick(pos, now, &mut output);
            let expected = power(15, day, kind);
            assert_eq!(
                read(&region, pos),
                detector(kind, expected),
                "day {day}, inverted {kind}"
            );
        }
    }
    // Noon and midnight, as a player sees them.
    assert_eq!(power(15, 6_000, false), 15);
    assert_eq!(power(15, 18_000, false), 0);
    assert_eq!(power(15, 18_000, true), 15 - (15 - sky_darken(18_000)));
}

#[test]
fn a_detector_reads_the_sky_only_on_a_twentieth_tick_of_game_time() {
    let pos = at(0, GROUND, 0);
    let mut region = open(&[(pos, detector(false, 0))]);
    region.clock.day = NOON;
    for game_time in [1, 19, 21, 39] {
        region.clock.game_time = game_time;
        let mut output = TickOutput::default();
        region.daylight_detector_tick(pos, detector(false, 0), &mut output);
        assert_eq!(read(&region, pos), detector(false, 0), "{game_time}");
    }
    region.clock.game_time = 60;
    let mut output = TickOutput::default();
    region.daylight_detector_tick(pos, detector(false, 0), &mut output);
    assert_eq!(read(&region, pos), detector(false, 15));
}

#[test]
fn a_detector_under_glass_gives_what_one_in_the_open_does_and_under_leaves_less() {
    // Glass lets the sky through whole. Leaves take one from it, and below them the
    // sky's light is like any other, one less a block (`clustine-light/tests/cases.rs`
    // 503): 14 in the leaves, 11 three blocks under them.
    for (cover, sky) in [
        (blocks::GLASS, 15),
        (state("minecraft:oak_leaves[persistent=true]"), 11),
    ] {
        let pos = at(0, GROUND, 0);
        let mut placed = vec![(pos, detector(false, 0))];
        placed.extend((-16..32).flat_map(|x| (-16..32).map(move |z| (at(x, ROOF, z), cover))));
        let mut region = open(&placed);
        assert_eq!(levels(&region, pos).map(|(sky, _)| sky), Some(sky));
        region.clock.game_time = 0;
        for day in [NOON, 9_000, 11_500] {
            region.clock.day = day;
            let mut output = TickOutput::default();
            let now = read(&region, pos);
            region.daylight_detector_tick(pos, now, &mut output);
            assert_eq!(
                power_of(read(&region, pos)),
                power(i32::from(sky), day, false)
            );
        }
    }
}

#[test]
fn a_click_inverts_a_detector_and_its_power_follows_in_the_same_tick() {
    let pos = at(2, GROUND, 0);
    let mut region = super::super::redstone::ticking_with(&[(pos, detector(false, 0))]);
    checked(&mut region, &super::super::redstone::holding(items::STICK));
    for inverted in [true, false] {
        checked(&mut region, &super::super::redstone::click(pos));
        let day = region.clock.day;
        let now = read(&region, pos);
        assert_eq!(
            now,
            detector(inverted, power(15, day, inverted)),
            "day {day}"
        );
    }
}
