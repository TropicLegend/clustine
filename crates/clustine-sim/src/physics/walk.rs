//! The game's walk through the blocks a box passes on its way along a move
//! (`BlockGetter.forEachBlockIntersectedBetween`, with `addCollisionsAlongTravel`,
//! `getFurthestCorner` and `BlockPos.betweenCornersInDirection`), ported line by
//! line: the order of the blocks and their step numbers decide which effects of the
//! blocks an entity is in share a step (`docs/groundwork/n4-brief.md`, section 1.9).

use std::collections::BTreeSet;

use clustine_world::BlockPos;

use super::aabb::Aabb;
use super::collide::axis_step_order;
use super::{Vec3, floor};

/// `Mth.sign` of a double.
fn sign(value: f64) -> i32 {
    if value == 0.0 {
        0
    } else if value > 0.0 {
        1
    } else {
        -1
    }
}

/// `Mth.frac`.
fn frac(value: f64) -> f64 {
    value - value.floor()
}

/// `BlockPos.betweenClosed` of a box: every block from the one holding its low corner
/// to the one holding its high corner, x fastest, then y, then z.
fn between_closed(bounds: &Aabb, mut visit: impl FnMut(BlockPos) -> bool) -> bool {
    let low = super::containing(bounds.min);
    let high = super::containing(bounds.max);
    for z in low.z.min(high.z)..=low.z.max(high.z) {
        for y in low.y.min(high.y)..=low.y.max(high.y) {
            for x in low.x.min(high.x)..=low.x.max(high.x) {
                if !visit(BlockPos::new(x, y, z)) {
                    return false;
                }
            }
        }
    }
    true
}

/// `BlockPos.betweenCornersInDirection(x0, y0, z0, x1, y1, z1, direction)`: the blocks
/// of the cuboid, from the corner the direction leaves, along the axes in
/// [`axis_step_order`], the last of them fastest.
fn between_corners_in_direction(low: [i32; 3], high: [i32; 3], direction: Vec3) -> Vec<BlockPos> {
    let min: [i32; 3] = std::array::from_fn(|axis| low[axis].min(high[axis]));
    let max: [i32; 3] = std::array::from_fn(|axis| low[axis].max(high[axis]));
    let size: [i32; 3] = std::array::from_fn(|axis| max[axis] - min[axis]);
    // `dcmpl ... iflt`: NaN counts as negative.
    let positive = |axis: usize| direction[axis] >= 0.0;
    let start: [i32; 3] =
        std::array::from_fn(|axis| if positive(axis) { min[axis] } else { max[axis] });
    let order = axis_step_order(direction);
    let step = |axis: usize| -> [i32; 3] {
        let mut step = [0; 3];
        step[axis] = if positive(axis) { 1 } else { -1 };
        step
    };
    let (first, second, third) = (step(order[0]), step(order[1]), step(order[2]));
    let (first_max, second_max, third_max) = (size[order[0]], size[order[1]], size[order[2]]);
    let mut blocks = Vec::new();
    for i in 0..=first_max {
        for j in 0..=second_max {
            for k in 0..=third_max {
                let at: [i32; 3] = std::array::from_fn(|axis| {
                    start[axis] + first[axis] * i + second[axis] * j + third[axis] * k
                });
                blocks.push(BlockPos::new(at[0], at[1], at[2]));
            }
        }
    }
    blocks
}

/// The same, of a box's corners rounded down.
fn between_box_corners(bounds: &Aabb, direction: Vec3) -> Vec<BlockPos> {
    between_corners_in_direction(bounds.min.map(floor), bounds.max.map(floor), direction)
}

/// `getFurthestCorner`, as the bytecode has it.
fn furthest_corner(delta: Vec3) -> [i32; 3] {
    let (x, y, z) = (delta[0].abs(), delta[1].abs(), delta[2].abs());
    let sign_of = |value: f64| if value >= 0.0 { 1 } else { -1 };
    let (sx, sy, sz) = (sign_of(delta[0]), sign_of(delta[1]), sign_of(delta[2]));
    if x <= y && x <= z {
        [-sx, -sz, sy]
    } else if y <= z {
        [sz, -sy, -sx]
    } else {
        [-sy, sx, -sz]
    }
}

fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}

/// `BlockGetter.forEachBlockIntersectedBetween(from, to, bounds, visitor)`: every block
/// the box `bounds` (where it ends) passes through on its way from `from` to `to`, each
/// once, with its step number; false where the visitor stopped the walk.
pub(crate) fn for_each_block_between(
    from: Vec3,
    to: Vec3,
    bounds: &Aabb,
    mut visit: impl FnMut(BlockPos, i32) -> bool,
) -> bool {
    let delta = super::sub(to, from);
    let tiny = f64::from(1.0e-5_f32 * 1.0e-5_f32);
    if super::length_sqr(delta) < tiny {
        return between_closed(bounds, |pos| visit(pos, 0));
    }
    let mut visited: BTreeSet<(i32, i32, i32)> = BTreeSet::new();
    let start = bounds.moved(super::scale(delta, -1.0));
    for pos in between_box_corners(&start, delta) {
        if !visit(pos, 0) {
            return false;
        }
        visited.insert(key(pos));
    }
    let last = along_travel(&mut visited, delta, bounds, &mut visit);
    if last < 0 {
        return false;
    }
    for pos in between_box_corners(bounds, delta) {
        if visited.insert(key(pos)) && !visit(pos, last + 1) {
            return false;
        }
    }
    true
}

/// `addCollisionsAlongTravel`: the blocks the box's leading corner crosses on its way,
/// numbered from 1; the last number, or -1 where the visitor stopped the walk.
fn along_travel(
    visited: &mut BTreeSet<(i32, i32, i32)>,
    delta: Vec3,
    bounds: &Aabb,
    visit: &mut impl FnMut(BlockPos, i32) -> bool,
) -> i32 {
    let size = bounds.size();
    let corner = furthest_corner(delta);
    let center = bounds.center();
    let end = [
        center[0] + size[0] * 0.5 * f64::from(corner[0]),
        center[1] + size[1] * 0.5 * f64::from(corner[1]),
        center[2] + size[2] * 0.5 * f64::from(corner[2]),
    ];
    let origin = super::sub(end, delta);
    let mut cell = origin.map(floor);
    let signs = delta.map(sign);
    let steps: [f64; 3] = std::array::from_fn(|axis| {
        if signs[axis] == 0 {
            f64::MAX
        } else {
            f64::from(signs[axis]) / delta[axis]
        }
    });
    let mut t: [f64; 3] = std::array::from_fn(|axis| {
        steps[axis]
            * if signs[axis] > 0 {
                1.0 - frac(origin[axis])
            } else {
                frac(origin[axis])
            }
    });
    let mut count = 0;
    while t[0] <= 1.0 || t[1] <= 1.0 || t[2] <= 1.0 {
        let axis = if t[0] < t[1] {
            if t[0] < t[2] { 0 } else { 2 }
        } else if t[1] < t[2] {
            1
        } else {
            2
        };
        cell[axis] += signs[axis];
        t[axis] += steps[axis];
        let block = Aabb {
            min: cell.map(f64::from),
            max: cell.map(|value| f64::from(value + 1)),
        };
        let Some(hit) = block.clip(origin, end) else {
            continue;
        };
        count += 1;
        let inside: [f64; 3] = std::array::from_fn(|axis| {
            let low = f64::from(cell[axis]) + f64::from(1.0e-5_f32);
            let high = f64::from(cell[axis]) + 1.0 - f64::from(1.0e-5_f32);
            // `Mth.clamp`: below the low bound the low bound, else at most the high.
            if hit[axis] < low {
                low
            } else {
                hit[axis].min(high)
            }
        });
        let other: [i32; 3] =
            std::array::from_fn(|axis| floor(inside[axis] - size[axis] * f64::from(corner[axis])));
        for pos in between_corners_in_direction(cell, other, delta) {
            if visited.insert(key(pos)) && !visit(pos, count) {
                return -1;
            }
        }
    }
    count
}

/// `BlockGetter.traverseBlocks`: the blocks a segment passes through, from the one it
/// starts in, until `hit` says one is hit; whether one was.
pub(crate) fn traverse(from: Vec3, to: Vec3, mut hit: impl FnMut(BlockPos) -> bool) -> bool {
    if from == to {
        return false;
    }
    let lerp = super::aabb::lerp;
    let end: [f64; 3] = std::array::from_fn(|axis| lerp(-1.0e-7, to[axis], from[axis]));
    let start: [f64; 3] = std::array::from_fn(|axis| lerp(-1.0e-7, from[axis], to[axis]));
    let mut cell = start.map(floor);
    if hit(BlockPos::new(cell[0], cell[1], cell[2])) {
        return true;
    }
    let delta = super::sub(end, start);
    let signs = delta.map(sign);
    let steps: [f64; 3] = std::array::from_fn(|axis| {
        if signs[axis] == 0 {
            f64::MAX
        } else {
            f64::from(signs[axis]) / delta[axis]
        }
    });
    let mut t: [f64; 3] = std::array::from_fn(|axis| {
        steps[axis]
            * if signs[axis] > 0 {
                1.0 - frac(start[axis])
            } else {
                frac(start[axis])
            }
    });
    while t[0] <= 1.0 || t[1] <= 1.0 || t[2] <= 1.0 {
        let axis = if t[0] < t[1] {
            if t[0] < t[2] { 0 } else { 2 }
        } else if t[1] < t[2] {
            1
        } else {
            2
        };
        cell[axis] += signs[axis];
        t[axis] += steps[axis];
        if hit(BlockPos::new(cell[0], cell[1], cell[2])) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walked(from: Vec3, to: Vec3, bounds: Aabb) -> Vec<(BlockPos, i32)> {
        let mut seen = Vec::new();
        for_each_block_between(from, to, &bounds, |pos, step| {
            seen.push((pos, step));
            true
        });
        seen
    }

    #[test]
    fn a_box_that_does_not_move_visits_the_blocks_it_is_in_at_step_0() {
        let bounds = Aabb::new([0.9, 1.0, 0.4], [1.1, 1.2, 0.6]);
        let seen = walked([1.0, 1.0, 0.5], [1.0, 1.0, 0.5], bounds);
        assert_eq!(
            seen,
            [(BlockPos::new(0, 1, 0), 0), (BlockPos::new(1, 1, 0), 0)]
        );
    }

    #[test]
    fn a_box_falling_through_three_blocks_visits_each_once_in_order() {
        let item = Aabb::of_entity([0.5, 1.2, 0.5], 0.25, 0.25).deflate(1.0e-5);
        let seen = walked([0.5, 3.6, 0.5], [0.5, 1.2, 0.5], item);
        let ys: Vec<i32> = seen.iter().map(|(pos, _)| pos.y).collect();
        assert_eq!(ys, [3, 2, 1]);
        let steps: Vec<i32> = seen.iter().map(|(_, step)| *step).collect();
        // The start's block at 0, then each block the leading corner enters.
        assert_eq!(steps[0], 0);
        assert!(steps.windows(2).all(|pair| pair[0] <= pair[1]), "{steps:?}");
    }
}
