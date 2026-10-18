//! The delta encoding of positions in an entity's relative moves: the game's
//! `VecDeltaCodec`. A position goes on the wire as the difference, in 4096ths of a block,
//! between it and a base both ends keep, each axis rounded on its own and the base
//! rounded the same way, so that the rounding of the one does not add to the other's.

/// How finely a delta is counted: 4096 steps to a block (`TRUNCATION_STEPS`).
const STEPS: f64 = 4096.0;

/// Java's `Math.round` of a double: the floor of the value and a half, halves going up
/// (to -2 from -2.5, where `f64::round` goes to -3), with what does not fit a long held
/// to its ends and not a number as 0, as Java's conversion has them.
pub fn java_round(value: f64) -> i64 {
    if value.is_nan() {
        return 0;
    }
    let floor = value.floor();
    // The fraction is exact: a double less its floor is one.
    let rounded = if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    };
    rounded as i64
}

/// A coordinate in steps (`VecDeltaCodec.encode`).
pub fn encode(value: f64) -> i64 {
    java_round(value * STEPS)
}

/// A count of steps as a coordinate (`VecDeltaCodec.decode`).
pub fn decode(steps: i64) -> f64 {
    steps as f64 / STEPS
}

/// How far a coordinate is from what a delta can say of it (`encodingPrecisionLoss`):
/// zero for one on a step.
pub fn precision_loss(value: f64) -> f64 {
    decode(encode(value)) - value
}

/// The base positions are sent relative to, as one end keeps it (`VecDeltaCodec`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VecDeltaCodec {
    pub base: [f64; 3],
}

impl VecDeltaCodec {
    pub fn new(base: [f64; 3]) -> Self {
        Self { base }
    }

    /// The steps from the base to `position` on each axis (`encodeX` and the others),
    /// which may be more than a delta holds.
    pub fn steps(&self, position: [f64; 3]) -> [i64; 3] {
        std::array::from_fn(|axis| encode(position[axis]) - encode(self.base[axis]))
    }

    /// The delta to `position`, or none if an axis is more than a short's worth of steps
    /// away (`tryEncode`, `isDeltaTooBig`): the game then sends the position whole.
    pub fn try_encode(&self, position: [f64; 3]) -> Option<[i16; 3]> {
        let steps = self.steps(position);
        let mut delta = [0; 3];
        for (into, steps) in delta.iter_mut().zip(steps) {
            *into = i16::try_from(steps).ok()?;
        }
        Some(delta)
    }

    /// The position a delta stands for, as a client reads it (`VecDeltaCodec.decode`):
    /// an axis that did not move keeps the base's coordinate exactly, one that moved
    /// lands on a step.
    pub fn decode(&self, delta: [i16; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| {
            if delta[axis] == 0 {
                self.base[axis]
            } else {
                decode(encode(self.base[axis]) + i64::from(delta[axis]))
            }
        })
    }

    /// The vector from the base to `position` (`delta`), whose length decides whether a
    /// move is worth sending.
    pub fn delta(&self, position: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| position[axis] - self.base[axis])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_rounds_halves_up_and_just_below_a_half_down() {
        assert_eq!(java_round(2.5), 3);
        assert_eq!(java_round(-2.5), -2);
        assert_eq!(java_round(-2.500_000_000_000_001), -3);
        assert_eq!(java_round(0.499_999_999_999_999_94), 0);
        assert_eq!(java_round(-0.5), 0);
        assert_eq!(java_round(f64::NAN), 0);
        assert_eq!(java_round(1e300), i64::MAX);
        assert_eq!(java_round(-1e300), i64::MIN);
    }

    #[test]
    fn a_delta_is_the_difference_of_the_two_positions_rounded_each_on_its_own() {
        let codec = VecDeltaCodec::new([0.5, -58.68, 0.5]);
        // -58.68 is -240353.28 steps, rounded to -240353; -58.64 is -240189.44, to
        // -240189: 164 apart, where the unrounded difference, 163.84, would be 164 too.
        assert_eq!(codec.try_encode([0.5, -58.64, 0.5]), Some([0, 164, 0]));
        // Half a step either side of the base's rounding: 0.000122 is 0.5 steps.
        let codec = VecDeltaCodec::new([0.0; 3]);
        assert_eq!(
            codec.try_encode([0.5 / 4096.0, 0.0, -0.5 / 4096.0]),
            Some([1, 0, 0])
        );
        // Eight blocks are 32768 steps, one more than a short holds; less a step fits.
        assert_eq!(codec.try_encode([8.0, 0.0, 0.0]), None);
        assert_eq!(codec.try_encode([-8.0, 0.0, 0.0]), Some([-32768, 0, 0]));
        assert_eq!(
            codec.try_encode([8.0 - 1.0 / 4096.0, 0.0, 0.0]),
            Some([32767, 0, 0])
        );
    }

    #[test]
    fn an_axis_that_did_not_move_keeps_the_base_and_one_that_did_lands_on_a_step() {
        let codec = VecDeltaCodec::new([0.3, 64.0, -0.3]);
        let [x, y, z] = codec.decode([0, 4096, 0]);
        assert_eq!((x, y, z), (0.3, 65.0, -0.3));
        let [x, ..] = codec.decode([1, 0, 0]);
        // 0.3 is 1228.8 steps, rounded to 1229; one more is 1230.
        assert_eq!(x, 1230.0 / 4096.0);
        assert!(precision_loss(0.3) != 0.0);
        assert_eq!(precision_loss(0.25), 0.0);
    }
}
