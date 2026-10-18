//! The game's `AABB`, as far as movement uses it.

use clustine_world::BlockPos;

use super::Vec3;

/// A box between two corners, each coordinate of `min` no greater than of `max`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Aabb {
    pub(crate) min: Vec3,
    pub(crate) max: Vec3,
}

impl Aabb {
    /// The box between two corners in any order, as the game's constructor sorts them.
    pub(crate) fn new(a: Vec3, b: Vec3) -> Self {
        Self {
            min: [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])],
            max: [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])],
        }
    }

    /// The box of one block.
    pub(crate) fn of_block(pos: BlockPos) -> Self {
        let min = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
        Self {
            min,
            max: [min[0] + 1.0, min[1] + 1.0, min[2] + 1.0],
        }
    }

    /// `EntityDimensions.makeBoundingBox` of an entity `width` wide and `height` high
    /// standing at `at`: the half width a float, both widened.
    pub(crate) fn of_entity(at: Vec3, width: f32, height: f32) -> Self {
        let half = f64::from(width / 2.0);
        let height = f64::from(height);
        Self::new(
            [at[0] - half, at[1], at[2] - half],
            [at[0] + half, at[1] + height, at[2] + half],
        )
    }

    /// `move`.
    pub(crate) fn moved(self, by: Vec3) -> Self {
        Self {
            min: [
                self.min[0] + by[0],
                self.min[1] + by[1],
                self.min[2] + by[2],
            ],
            max: [
                self.max[0] + by[0],
                self.max[1] + by[1],
                self.max[2] + by[2],
            ],
        }
    }

    /// `expandTowards`: a negative component lowers the minimum, a positive one raises
    /// the maximum.
    pub(crate) fn expand_towards(self, by: Vec3) -> Self {
        let mut expanded = self;
        for (axis, along) in by.into_iter().enumerate() {
            if along < 0.0 {
                expanded.min[axis] += along;
            } else if along > 0.0 {
                expanded.max[axis] += along;
            }
        }
        Self::new(expanded.min, expanded.max)
    }

    /// `inflate(x, y, z)`, through the sorting constructor.
    pub(crate) fn inflate(self, by: Vec3) -> Self {
        Self::new(
            [
                self.min[0] - by[0],
                self.min[1] - by[1],
                self.min[2] - by[2],
            ],
            [
                self.max[0] + by[0],
                self.max[1] + by[1],
                self.max[2] + by[2],
            ],
        )
    }

    /// `deflate(d)`, which is `inflate(-d)`.
    pub(crate) fn deflate(self, by: f64) -> Self {
        self.inflate([-by, -by, -by])
    }

    /// `intersects`: overlapping on all three axes, strictly.
    pub(crate) fn intersects(self, other: Aabb) -> bool {
        (0..3).all(|axis| self.min[axis] < other.max[axis] && self.max[axis] > other.min[axis])
    }

    /// `contains(Vec3)`: half open, the minimum in and the maximum out.
    pub(crate) fn contains(self, point: Vec3) -> bool {
        (0..3).all(|axis| point[axis] >= self.min[axis] && point[axis] < self.max[axis])
    }

    /// `getCenter`.
    pub(crate) fn center(self) -> Vec3 {
        std::array::from_fn(|axis| lerp(0.5, self.min[axis], self.max[axis]))
    }

    /// `getXsize`, `getYsize` and `getZsize`.
    pub(crate) fn size(self) -> Vec3 {
        std::array::from_fn(|axis| self.max[axis] - self.min[axis])
    }

    /// The static `AABB.clip`: where the segment from `from` to `to` first enters the
    /// box through one of its faces, within the segment, with the game's tolerance of
    /// 1e-7 across the face.
    pub(crate) fn clip(self, from: Vec3, to: Vec3) -> Option<Vec3> {
        let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let mut nearest = 1.0;
        let mut hit = false;
        // Each axis in the game's order, the face the segment meets first along it:
        // the near one, by the sign of the delta.
        for (axis, other_a, other_b) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
            let side = if delta[axis] > 1.0e-7 {
                self.min[axis]
            } else if delta[axis] < -1.0e-7 {
                self.max[axis]
            } else {
                continue;
            };
            // `clipPoint`.
            let d = (side - from[axis]) / delta[axis];
            let e = from[other_a] + d * delta[other_a];
            let f = from[other_b] + d * delta[other_b];
            if 0.0 < d
                && d < nearest
                && self.min[other_a] - 1.0e-7 < e
                && e < self.max[other_a] + 1.0e-7
                && self.min[other_b] - 1.0e-7 < f
                && f < self.max[other_b] + 1.0e-7
            {
                nearest = d;
                hit = true;
            }
        }
        hit.then(|| {
            [
                from[0] + nearest * delta[0],
                from[1] + nearest * delta[1],
                from[2] + nearest * delta[2],
            ]
        })
    }

    /// `collidedAlongVector`: whether this box, moved by `vector`, meets any of
    /// `boxes`: each grown by half this box's size less 1e-7, and tested for holding the
    /// centre before or after the move, or being crossed by it.
    pub(crate) fn collided_along_vector(self, vector: Vec3, boxes: &[Aabb]) -> bool {
        let start = self.center();
        let end = [
            start[0] + vector[0],
            start[1] + vector[1],
            start[2] + vector[2],
        ];
        let size = self.size();
        let grow = [
            size[0] * 0.5 - 1.0e-7,
            size[1] * 0.5 - 1.0e-7,
            size[2] * 0.5 - 1.0e-7,
        ];
        boxes.iter().any(|other| {
            let grown = other.inflate(grow);
            grown.contains(end) || grown.contains(start) || grown.clip(start, end).is_some()
        })
    }
}

/// `Mth.lerp` of doubles.
pub(crate) fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_s_box_is_a_quarter_wide_and_high_about_its_feet() {
        let item = Aabb::of_entity([0.5, 64.0, 0.5], 0.25, 0.25);
        assert_eq!(item.min, [0.375, 64.0, 0.375]);
        assert_eq!(item.max, [0.625, 64.25, 0.625]);
    }

    #[test]
    fn a_segment_into_a_box_meets_its_near_face() {
        let block = Aabb::of_block(BlockPos::new(0, 0, 0));
        assert_eq!(
            block.clip([-1.0, 0.5, 0.5], [0.5, 0.5, 0.5]),
            Some([0.0, 0.5, 0.5])
        );
        // From inside nothing is met: the game's clip looks for a face ahead.
        assert_eq!(block.clip([0.5, 0.5, 0.5], [2.0, 0.5, 0.5]), None);
        assert_eq!(block.clip([-1.0, 2.0, 0.5], [0.5, 2.0, 0.5]), None);
    }
}
