// Adapted from SteelMC's `steel-utils/src/direction.rs` (`ordered_by_nearest`,
// `from_yaw`) and `steel-math/src/angle.rs` (`convert_to_rotation_segment`).

//! Directions, the player's facing and the shapes of blocks, as vanilla computes them.
//!
//! The sides of a block are [`clustine_data::Face`], here called [`Dir`], whose order is
//! the game's. What vanilla's `Direction` can do and the data crate's type cannot is
//! [`DirExt`].

use clustine_data::Face as Dir;
use clustine_noise::trig;
use clustine_world::BlockPos;

/// The four horizontal sides in vanilla's `Direction.Plane.HORIZONTAL` order.
pub(crate) const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

/// An axis of the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    /// The value of an `axis` property.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        }
    }
}

/// What vanilla's `Direction` offers beyond the data crate's [`Dir`].
pub(crate) trait DirExt: Sized {
    fn step(self) -> [i32; 3];
    fn relative(self, pos: BlockPos) -> BlockPos;
    fn axis(self) -> Axis;
    fn is_horizontal(&self) -> bool;
    /// Turned a quarter clockwise about the vertical, as seen from above.
    fn clockwise(self) -> Self;
    fn counter_clockwise(self) -> Self;
    /// The value of a `facing` property.
    fn name(self) -> &'static str;
    fn from_name(name: &str) -> Option<Self>;
}

impl DirExt for Dir {
    fn step(self) -> [i32; 3] {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
        }
    }

    fn relative(self, pos: BlockPos) -> BlockPos {
        let [x, y, z] = self.step();
        pos.offset(x, y, z)
    }

    fn axis(self) -> Axis {
        match self {
            Dir::Down | Dir::Up => Axis::Y,
            Dir::North | Dir::South => Axis::Z,
            Dir::West | Dir::East => Axis::X,
        }
    }

    fn is_horizontal(&self) -> bool {
        self.axis() != Axis::Y
    }

    fn clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            vertical => vertical,
        }
    }

    fn counter_clockwise(self) -> Dir {
        match self {
            Dir::North => Dir::West,
            Dir::West => Dir::South,
            Dir::South => Dir::East,
            Dir::East => Dir::North,
            vertical => vertical,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }

    fn from_name(name: &str) -> Option<Dir> {
        Dir::ALL.into_iter().find(|dir| dir.name() == name)
    }
}

/// The side of the contract's [`crate::api::Face`].
pub(crate) fn from_api(face: crate::api::Face) -> Dir {
    use crate::api::Face;
    match face {
        Face::Bottom => Dir::Down,
        Face::Top => Dir::Up,
        Face::North => Dir::North,
        Face::South => Dir::South,
        Face::West => Dir::West,
        Face::East => Dir::East,
    }
}

/// The contract's [`crate::api::Face`] of a side.
pub(crate) fn to_api(dir: Dir) -> crate::api::Face {
    use crate::api::Face;
    match dir {
        Dir::Down => Face::Bottom,
        Dir::Up => Face::Top,
        Dir::North => Face::North,
        Dir::South => Face::South,
        Dir::West => Face::West,
        Dir::East => Face::East,
    }
}

/// Vanilla's `Direction.orderedByNearest`: the six sides by how directly a player with
/// this rotation looks towards each. It takes its sines from the game's table, as the
/// game does: the mathematics library rounds differently at 45 degrees.
pub(crate) fn ordered_by_nearest(yaw: f32, pitch: f32) -> [Dir; 6] {
    const RADIANS_PER_DEGREE: f32 = std::f64::consts::PI as f32 / 180.0;
    let pitch = pitch * RADIANS_PER_DEGREE;
    let yaw = -yaw * RADIANS_PER_DEGREE;
    let pitch_sin = trig::sin(f64::from(pitch));
    let pitch_cos = trig::cos(f64::from(pitch));
    let yaw_sin = trig::sin(f64::from(yaw));
    let yaw_cos = trig::cos(f64::from(yaw));
    let x_positive = yaw_sin > 0.0;
    let y_positive = pitch_sin < 0.0;
    let z_positive = yaw_cos > 0.0;
    let x_yaw = if x_positive { yaw_sin } else { -yaw_sin };
    let y_magnitude = if y_positive { -pitch_sin } else { pitch_sin };
    let z_yaw = if z_positive { yaw_cos } else { -yaw_cos };
    let x_magnitude = x_yaw * pitch_cos;
    let z_magnitude = z_yaw * pitch_cos;
    let axis_x = if x_positive { Dir::East } else { Dir::West };
    let axis_y = if y_positive { Dir::Up } else { Dir::Down };
    let axis_z = if z_positive { Dir::South } else { Dir::North };
    if x_yaw > z_yaw {
        if y_magnitude > x_magnitude {
            direction_array(axis_y, axis_x, axis_z)
        } else if z_magnitude > y_magnitude {
            direction_array(axis_x, axis_z, axis_y)
        } else {
            direction_array(axis_x, axis_y, axis_z)
        }
    } else if y_magnitude > z_magnitude {
        direction_array(axis_y, axis_z, axis_x)
    } else if x_magnitude > y_magnitude {
        direction_array(axis_z, axis_x, axis_y)
    } else {
        direction_array(axis_z, axis_y, axis_x)
    }
}

/// Vanilla's `Direction.makeDirectionArray`: the three nearest, then their opposites
/// from the last to the first.
fn direction_array(first: Dir, second: Dir, third: Dir) -> [Dir; 6] {
    [
        first,
        second,
        third,
        third.opposite(),
        second.opposite(),
        first.opposite(),
    ]
}

/// Vanilla's `Direction.fromYRot`: the horizontal side a player with this yaw faces.
pub(crate) fn from_y_rot(yaw: f32) -> Dir {
    match ((f64::from(yaw) / 90.0 + 0.5).floor() as i64) & 3 {
        0 => Dir::South,
        1 => Dir::West,
        2 => Dir::North,
        _ => Dir::East,
    }
}

/// The yaw of a horizontal side, vanilla's `Direction.toYRot`.
pub(crate) fn y_rot(dir: Dir) -> f32 {
    match dir {
        Dir::South => 0.0,
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        Dir::Down | Dir::Up => 0.0,
    }
}

/// Vanilla's `RotationSegment.convertToSegment`: an angle in sixteenths of a turn, the
/// value of a `rotation` property.
pub(crate) fn rotation_segment(degrees: f32) -> u8 {
    // `Math.round` of a float is the floor of it plus a half, in floats.
    ((degrees * (16.0 / 360.0) + 0.5).floor() as i64 & 15) as u8
}

/// Vanilla's `RotationSegment.convertToDirection`: the side a rotation points to, if it
/// points to one of the four.
pub(crate) fn segment_direction(segment: u8) -> Option<Dir> {
    match segment {
        0 => Some(Dir::South),
        4 => Some(Dir::West),
        8 => Some(Dir::North),
        12 => Some(Dir::East),
        _ => None,
    }
}

pub(crate) use clustine_data::faces::{face_covers, face_full};

/// The boxes of a shape moved to the block at `pos`.
pub(crate) fn moved(shape: clustine_data::Shape, pos: BlockPos) -> Vec<[f64; 6]> {
    let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
    shape
        .boxes()
        .map(|b| [b[0] + x, b[1] + y, b[2] + z, b[3] + x, b[4] + y, b[5] + z])
        .collect()
}

/// Whether two boxes share some volume; boxes that only touch do not.
pub(crate) fn overlaps(a: &[f64; 6], b: &[f64; 6]) -> bool {
    a[0] < b[3] && a[3] > b[0] && a[1] < b[4] && a[4] > b[1] && a[2] < b[5] && a[5] > b[2]
}

/// Where a ray from `from` to `to` first enters one of `boxes`: the fraction of the way,
/// and the side of the box it enters by. Vanilla's `AABB.clip` over a shape's boxes.
pub(crate) fn clip_boxes(boxes: &[[f64; 6]], from: [f64; 3], to: [f64; 3]) -> Option<(f64, Dir)> {
    let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut best: Option<(f64, Dir)> = None;
    for b in boxes {
        let mut enter = f64::NEG_INFINITY;
        let mut leave = f64::INFINITY;
        let mut side = Dir::Down;
        let mut missed = false;
        for axis in 0..3 {
            let (low, high) = (b[axis], b[axis + 3]);
            if delta[axis] == 0.0 {
                if from[axis] < low || from[axis] > high {
                    missed = true;
                }
                continue;
            }
            let t_low = (low - from[axis]) / delta[axis];
            let t_high = (high - from[axis]) / delta[axis];
            let (near, far, near_side) = if t_low < t_high {
                (t_low, t_high, negative_side(axis))
            } else {
                (t_high, t_low, negative_side(axis).opposite())
            };
            if near > enter {
                enter = near;
                side = near_side;
            }
            leave = leave.min(far);
        }
        if missed || enter > leave || !(0.0..=1.0).contains(&enter) {
            continue;
        }
        if best.is_none_or(|(t, _)| enter < t) {
            best = Some((enter, side));
        }
    }
    best
}

/// The side of a block that faces towards negative coordinates on `axis`, 0 being x.
fn negative_side(axis: usize) -> Dir {
    match axis {
        0 => Dir::West,
        1 => Dir::Down,
        _ => Dir::North,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_looking_straight_down_has_down_nearest_and_up_last() {
        let order = ordered_by_nearest(0.0, 90.0);
        assert_eq!(order[0], Dir::Down);
        assert_eq!(order[5], Dir::Up);
    }

    #[test]
    fn a_player_looking_level_to_the_south_has_south_nearest() {
        assert_eq!(ordered_by_nearest(0.0, 0.0)[0], Dir::South);
        assert_eq!(ordered_by_nearest(90.0, 0.0)[0], Dir::West);
        assert_eq!(ordered_by_nearest(180.0, 10.0)[0], Dir::North);
        assert_eq!(ordered_by_nearest(-90.0, -10.0)[0], Dir::East);
        assert_eq!(ordered_by_nearest(-90.0, -80.0)[0], Dir::Up);
    }

    #[test]
    fn every_order_holds_each_side_once_and_pairs_opposites_from_the_ends() {
        for yaw in (-360..=360).step_by(15) {
            for pitch in (-90..=90).step_by(15) {
                let order = ordered_by_nearest(yaw as f32, pitch as f32);
                for dir in Dir::ALL {
                    assert_eq!(order.iter().filter(|&&d| d == dir).count(), 1);
                }
                for i in 0..3 {
                    assert_eq!(order[i].opposite(), order[5 - i]);
                }
            }
        }
    }

    #[test]
    fn the_facing_from_the_yaw_turns_at_the_diagonals() {
        assert_eq!(from_y_rot(0.0), Dir::South);
        assert_eq!(from_y_rot(44.9), Dir::South);
        assert_eq!(from_y_rot(45.0), Dir::West);
        assert_eq!(from_y_rot(180.0), Dir::North);
        assert_eq!(from_y_rot(-90.0), Dir::East);
        assert_eq!(from_y_rot(-135.0), Dir::East);
        assert_eq!(from_y_rot(-136.0), Dir::North);
        assert_eq!(from_y_rot(720.0), Dir::South);
    }

    #[test]
    fn rotation_segments_are_sixteenths_of_a_turn_rounded() {
        assert_eq!(rotation_segment(0.0), 0);
        assert_eq!(rotation_segment(11.24), 0);
        assert_eq!(rotation_segment(11.25), 1);
        assert_eq!(rotation_segment(180.0), 8);
        assert_eq!(rotation_segment(-90.0), 12);
        assert_eq!(rotation_segment(359.0), 0);
    }

    #[test]
    fn a_ray_enters_a_block_by_the_side_it_comes_from() {
        let block = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        let (t, side) = clip_boxes(&block, [0.5, 3.0, 0.5], [0.5, -1.0, 0.5]).unwrap();
        assert_eq!(side, Dir::Up);
        assert!((t - 0.5).abs() < 1e-9);
        let (_, side) = clip_boxes(&block, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5]).unwrap();
        assert_eq!(side, Dir::West);
        assert!(clip_boxes(&block, [2.0, 0.5, 0.5], [3.0, 0.5, 0.5]).is_none());
    }
}
