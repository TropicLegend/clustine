//! The game's `VoxelShape` placed in the world: a shape's cells between its boundaries,
//! moved by an offset (the game's `OffsetDoubleList`), and what the collider asks of it
//! (`VoxelShape.collide`, `Shapes.joinIsNotEmpty` with a box, its boxes).

use clustine_data::collisions::{Axis, VoxelShape};

use super::Vec3;
use super::aabb::Aabb;

/// What a placed shape is made of: one of the game's shapes from the data, or one box
/// of the game's own that no block state has as its shape (powder snow's while
/// something falls into it, scaffolding's bottom while one stands below its top).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Cells {
    Data(VoxelShape),
    /// One filled cell between these bounds of the block, as `Shapes.box` makes a box
    /// that is not on the grid.
    Box([f64; 6]),
}

/// A shape moved into the world: each boundary is the shape's own plus `offset` (and,
/// for a shape a block shifts by its position, plus that shift first).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placed {
    pub(crate) cells: Cells,
    /// The shift of a block by its position (bamboo, pointed dripstone), added to the
    /// shape's boundaries before the block's position, as the game moves it twice.
    pub(crate) shift: Option<Vec3>,
    pub(crate) offset: Vec3,
}

fn axis_of(index: usize) -> Axis {
    Axis::ALL[index]
}

impl Placed {
    /// The number of cells along `axis`.
    pub(crate) fn size(&self, axis: usize) -> usize {
        match self.cells {
            Cells::Data(shape) => shape.size(axis_of(axis)),
            Cells::Box(_) => 1,
        }
    }

    /// The boundary `index` along `axis`, moved.
    pub(crate) fn coord(&self, axis: usize, index: usize) -> f64 {
        let own = match self.cells {
            Cells::Data(shape) => shape.coord(axis_of(axis), index),
            Cells::Box(bounds) => bounds[axis + 3 * index],
        };
        match self.shift {
            Some(shift) => (own + shift[axis]) + self.offset[axis],
            None => own + self.offset[axis],
        }
    }

    /// `DiscreteVoxelShape.isFullWide`: a cell outside the shape is not full.
    pub(crate) fn is_full(&self, x: i64, y: i64, z: i64) -> bool {
        if x < 0 || y < 0 || z < 0 {
            return false;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        match self.cells {
            Cells::Data(shape) => shape.is_full(x, y, z),
            Cells::Box(_) => x == 0 && y == 0 && z == 0,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        match self.cells {
            Cells::Data(shape) => shape.is_empty(),
            Cells::Box(_) => false,
        }
    }

    /// `findIndex`: the last boundary at or below `value`, -1 where there is none (the
    /// game's binary search).
    fn find_index(&self, axis: usize, value: f64) -> i64 {
        // `Mth.binarySearch(0, size + 1, i -> value < coord(i)) - 1`.
        let (mut low, mut high) = (0_i64, self.size(axis) as i64 + 1);
        let mut length = high - low;
        while length > 0 {
            let half = length / 2;
            let middle = low + half;
            if value < self.coord(axis, middle as usize) {
                length = half;
            } else {
                low = middle + 1;
                length -= half + 1;
            }
        }
        high = low;
        high - 1
    }

    /// The cell in the frame of `axis`: `(a, b, c)` for the axis moved along and the two
    /// after it in the game's cycle, back in x, y and z.
    fn full_in_frame(&self, axis: usize, a: i64, b: i64, c: i64) -> bool {
        let mut cell = [0_i64; 3];
        cell[axis] = a;
        cell[(axis + 1) % 3] = b;
        cell[(axis + 2) % 3] = c;
        self.is_full(cell[0], cell[1], cell[2])
    }

    /// `VoxelShape.collide(axis, box, d)`: how far of `d` the box can move along `axis`
    /// before it meets a full cell of this shape, looking from the box's far side on
    /// along the axis and stopping at the first full cell in the box's cross-section,
    /// whether or not it shortens the move.
    pub(crate) fn collide(&self, axis: usize, bounds: &Aabb, mut d: f64) -> f64 {
        if self.is_empty() {
            return d;
        }
        if d.abs() < 1.0e-7 {
            return 0.0;
        }
        let (b_axis, c_axis) = ((axis + 1) % 3, (axis + 2) % 3);
        let max_a = bounds.max[axis];
        let min_a = bounds.min[axis];
        let low = self.find_index(axis, min_a + 1.0e-7);
        let high = self.find_index(axis, max_a - 1.0e-7);
        let b0 = 0.max(self.find_index(b_axis, bounds.min[b_axis] + 1.0e-7));
        let b1 = (self.size(b_axis) as i64)
            .min(self.find_index(b_axis, bounds.max[b_axis] - 1.0e-7) + 1);
        let c0 = 0.max(self.find_index(c_axis, bounds.min[c_axis] + 1.0e-7));
        let c1 = (self.size(c_axis) as i64)
            .min(self.find_index(c_axis, bounds.max[c_axis] - 1.0e-7) + 1);
        let n = self.size(axis) as i64;
        if d > 0.0 {
            for i in high + 1..n {
                for j in b0..b1 {
                    for k in c0..c1 {
                        if self.full_in_frame(axis, i, j, k) {
                            let e = self.coord(axis, i as usize) - max_a;
                            if e >= -1.0e-7 {
                                d = d.min(e);
                            }
                            return d;
                        }
                    }
                }
            }
        } else if d < 0.0 {
            let mut i = low - 1;
            while i >= 0 {
                for j in b0..b1 {
                    for k in c0..c1 {
                        if self.full_in_frame(axis, i, j, k) {
                            let e = self.coord(axis, (i + 1) as usize) - min_a;
                            if e <= 1.0e-7 {
                                d = d.max(e);
                            }
                            return d;
                        }
                    }
                }
                i -= 1;
            }
        }
        d
    }

    /// `VoxelShape.min(axis)` and `max(axis)`: the bounds of its full cells.
    fn bounds_along(&self, axis: usize) -> (f64, f64) {
        let size = self.size(axis);
        let (mut first, mut last) = (None, None);
        for i in 0..size {
            let any = (0..self.size((axis + 1) % 3)).any(|j| {
                (0..self.size((axis + 2) % 3))
                    .any(|k| self.full_in_frame(axis, i as i64, j as i64, k as i64))
            });
            if any {
                first.get_or_insert(i);
                last = Some(i + 1);
            }
        }
        match (first, last) {
            (Some(first), Some(last)) => (self.coord(axis, first), self.coord(axis, last)),
            _ => (f64::INFINITY, f64::NEG_INFINITY),
        }
    }

    /// `Shapes.joinIsNotEmpty(this, Shapes.create(bounds), AND)`: whether a full cell of
    /// the shape and the box overlap by more than the game's 1e-7 on every axis, as the
    /// game merges the two grids. A box thinner than 1e-7 along an axis is empty.
    pub(crate) fn overlaps_box(&self, bounds: &Aabb) -> bool {
        let size = bounds.size();
        if size.iter().any(|extent| *extent < 1.0e-7) || self.is_empty() {
            return false;
        }
        for axis in 0..3 {
            let (min, max) = self.bounds_along(axis);
            if max < bounds.min[axis] - 1.0e-7 || bounds.max[axis] < min - 1.0e-7 {
                return false;
            }
        }
        let merged: [Vec<(i64, i64)>; 3] = std::array::from_fn(|axis| {
            let ours: Vec<f64> = (0..=self.size(axis))
                .map(|index| self.coord(axis, index))
                .collect();
            merge(&ours, &[bounds.min[axis], bounds.max[axis]])
        });
        // The box is one full cell: its index 0.
        for (x, bx) in &merged[0] {
            if *bx != 0 {
                continue;
            }
            for (y, by) in &merged[1] {
                if *by != 0 {
                    continue;
                }
                for (z, bz) in &merged[2] {
                    if *bz == 0 && self.is_full(*x, *y, *z) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The boxes of the shape's full cells, one each (the game's `toAabbs` joins cells
    /// into larger boxes, which covers the same space: what is met along a vector is the
    /// same).
    pub(crate) fn boxes(&self) -> Vec<Aabb> {
        let mut boxes = Vec::new();
        for x in 0..self.size(0) {
            for y in 0..self.size(1) {
                for z in 0..self.size(2) {
                    if self.is_full(x as i64, y as i64, z as i64) {
                        boxes.push(Aabb::new(
                            [self.coord(0, x), self.coord(1, y), self.coord(2, z)],
                            [
                                self.coord(0, x + 1),
                                self.coord(1, y + 1),
                                self.coord(2, z + 1),
                            ],
                        ));
                    }
                }
            }
        }
        boxes
    }
}

/// The game's `IndirectMerger` for an AND of two shapes (neither side alone matters):
/// the cells of the merged grid, each as the pair of cells of the first and the second
/// list it lies in; boundaries closer than 1e-7 are one.
fn merge(first: &[f64], second: &[f64]) -> Vec<(i64, i64)> {
    let (sizes_a, sizes_b) = (first.len(), second.len());
    let mut pairs: Vec<(i64, i64)> = Vec::new();
    let mut last = f64::NAN;
    let (mut m, mut n) = (0_usize, 0_usize);
    loop {
        let a_done = m >= sizes_a;
        let b_done = n >= sizes_b;
        if a_done && b_done {
            break;
        }
        let take_first = !a_done && (b_done || first[m] < second[n] + 1.0e-7);
        if take_first {
            m += 1;
            if n == 0 || b_done {
                continue;
            }
        } else {
            n += 1;
            if m == 0 || a_done {
                continue;
            }
        }
        let (i, j) = (m as i64 - 1, n as i64 - 1);
        let value = if take_first {
            first[m - 1]
        } else {
            second[n - 1]
        };
        // `!(last >= value - 1e-7)`, false while `last` is NaN.
        if last >= value - 1.0e-7 {
            if let Some(previous) = pairs.last_mut() {
                *previous = (i, j);
            }
        } else {
            pairs.push((i, j));
            last = value;
        }
    }
    // `forMergedIndexes` gives every pair but the last, which closes the grid.
    pairs.pop();
    pairs
}

#[cfg(test)]
mod tests {
    use clustine_data::BlockState;

    use super::*;

    fn placed(name: &str, at: [f64; 3]) -> Placed {
        Placed {
            cells: Cells::Data(BlockState::parse(name).unwrap().voxel_collision_shape()),
            shift: None,
            offset: at,
        }
    }

    #[test]
    fn a_box_falling_onto_a_slab_stops_at_its_top() {
        let slab = placed("minecraft:oak_slab[type=bottom]", [0.0, 0.0, 0.0]);
        let item = Aabb::of_entity([0.5, 1.0, 0.5], 0.25, 0.25);
        assert_eq!(slab.collide(1, &item, -0.7), -0.5);
        // Beside it, nothing is in the way.
        let beside = Aabb::of_entity([1.5, 1.0, 0.5], 0.25, 0.25);
        assert_eq!(slab.collide(1, &beside, -0.7), -0.7);
        // A move short of it is not shortened.
        assert_eq!(slab.collide(1, &item, -0.3), -0.3);
    }

    #[test]
    fn the_overlap_of_a_shape_and_a_box_must_be_more_than_the_tolerance() {
        let stone = placed("minecraft:stone", [0.0, 0.0, 0.0]);
        let touching = Aabb::new([0.0, 1.0, 0.0], [1.0, 2.0, 1.0]);
        assert!(!stone.overlaps_box(&touching));
        let into = Aabb::new([0.0, 0.5, 0.0], [1.0, 2.0, 1.0]);
        assert!(stone.overlaps_box(&into));
        let barely = Aabb::new([0.0, 1.0 - 5.0e-8, 0.0], [1.0, 2.0, 1.0]);
        assert!(!stone.overlaps_box(&barely));
        let slab = placed("minecraft:oak_slab[type=bottom]", [0.0, 0.0, 0.0]);
        assert!(!slab.overlaps_box(&Aabb::new([0.0, 0.6, 0.0], [1.0, 0.9, 1.0])));
        assert!(slab.overlaps_box(&Aabb::new([0.0, 0.4, 0.0], [1.0, 0.9, 1.0])));
    }
}
