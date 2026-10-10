// Adapted from SteelMC's `steel-worldgen/src/density/spline_eval.rs`.

//! Evaluation of the game's cubic splines (`CubicSpline.Multipoint`).
//!
//! The shape of the overworld is a handful of nested splines: how high the land is
//! and how sharply it rises as functions of continentalness, erosion and ridges. The
//! functions here work on plain slices, so that the generated routers can keep each
//! spline's points in static tables and nest splines by passing a closure.

/// The index of the last location not above `input`, or -1 where `input` is below
/// them all. `locations` ascend.
#[inline]
#[must_use]
pub fn find_interval(locations: &[f32], input: f32) -> i32 {
    let mut low = 0_usize;
    let mut high = locations.len();
    while low < high {
        let middle = low + (high - low) / 2;
        if input < locations[middle] {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low as i32 - 1
}

/// The cubic between two neighbouring points of a spline, `(x1, y1)` with slope `d1`
/// and `(x2, y2)` with slope `d2`, at `input`.
///
/// The game's formula: `lerp(t, y1, y2) + t * (1 - t) * lerp(t, a, b)`.
#[inline]
#[must_use]
pub fn hermite_interpolate(
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
    d1: f32,
    d2: f32,
    input: f32,
) -> f32 {
    let t = (input - x1) / (x2 - x1);
    let width = x2 - x1;
    let a = d1 * width - (y2 - y1);
    let b = -d2 * width + (y2 - y1);
    let between_values = y1 + t * (y2 - y1);
    let between_slopes = a + t * (b - a);
    between_values + t * (1.0 - t) * between_slopes
}

/// A spline at `input`.
///
/// `locations` and `derivatives` are as long as the spline has points. `value_at`
/// gives the value at a point, which is a constant or another spline's value. Outside
/// its points a spline goes on straight with the slope of its end. A spline without
/// points is zero.
#[inline]
pub fn evaluate_spline(
    locations: &[f32],
    derivatives: &[f32],
    input: f32,
    value_at: impl Fn(usize) -> f32,
) -> f32 {
    if locations.is_empty() {
        return 0.0;
    }

    let last = locations.len() - 1;
    let start = find_interval(locations, input);
    if start < 0 {
        return value_at(0) + derivatives[0] * (input - locations[0]);
    }
    let start = start as usize;
    if start == last {
        return value_at(last) + derivatives[last] * (input - locations[last]);
    }

    hermite_interpolate(
        locations[start],
        locations[start + 1],
        value_at(start),
        value_at(start + 1),
        derivatives[start],
        derivatives[start + 1],
        input,
    )
}

#[cfg(test)]
mod tests {
    // SteelMC's cases, with exact results where the arithmetic is exact.

    use super::*;

    #[test]
    fn the_interval_is_found_before_between_at_and_after_the_points() {
        let locations = [0.0, 1.0, 2.0];
        assert_eq!(find_interval(&locations, -1.0), -1);
        assert_eq!(find_interval(&locations, 0.5), 0);
        assert_eq!(find_interval(&locations, 1.0), 1);
        assert_eq!(find_interval(&locations, 3.0), 2);
        assert_eq!(find_interval(&[], 3.0), -1);
    }

    #[test]
    fn a_cubic_with_the_slope_of_its_chord_is_a_line() {
        assert_eq!(
            hermite_interpolate(0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.25),
            0.25
        );
    }

    #[test]
    fn a_cubic_with_flat_ends_passes_through_the_middle() {
        assert_eq!(hermite_interpolate(0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.5), 0.5);
        // a = -1, b = 1: 0.25 + 0.25 * 0.75 * (-1 + 0.25 * 2) = 0.15625.
        assert_eq!(
            hermite_interpolate(0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.25),
            0.156_25
        );
    }

    #[test]
    fn a_spline_goes_on_straight_beyond_its_ends() {
        let values = [0.0_f32, 1.0];
        let before = evaluate_spline(&[0.0, 1.0], &[2.0, 0.0], -1.0, |i| values[i]);
        assert_eq!(before, -2.0);
        let after = evaluate_spline(&[0.0, 1.0], &[0.0, 3.0], 2.0, |i| values[i]);
        assert_eq!(after, 4.0);
    }

    #[test]
    fn a_spline_of_equal_values_and_no_slope_is_flat() {
        let flat = evaluate_spline(&[0.0, 1.0, 2.0], &[0.0; 3], 0.5, |_| 1.0);
        assert_eq!(flat, 1.0);
        assert_eq!(evaluate_spline(&[], &[], 0.5, |_| 1.0), 0.0);
    }

    #[test]
    fn a_spline_can_take_its_values_from_another() {
        let inner = |input: f32| evaluate_spline(&[0.0, 1.0], &[0.0, 0.0], input, |i| i as f32);
        let outer = evaluate_spline(&[0.0, 1.0], &[0.0, 0.0], 0.5, |i| inner(i as f32) * 2.0);
        assert_eq!(outer, 1.0);
    }
}
