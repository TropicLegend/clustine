//! Whether a shape covers a face of its block, as the game's tests of a face against a
//! shape find. Placing asks it of the block a torch or a snow layer would rest on,
//! and the search for a new world's spawn of the block a player would stand on.

use crate::Face;

/// How far inside a face vanilla takes a shape's slice for the face's shape
/// (`VoxelShape.getFaceShape`).
const FACE_EPSILON: f64 = 1.0e-7;

/// The rectangles that `boxes` cover of `face`, each `[min u, min v, max u, max v]` in
/// the two axes that lie in the face, in the order x, y, z. A box counts where it
/// crosses the plane just inside the face, so that a fence's post, which reaches above
/// its block, covers the top of it.
fn face_rectangles(boxes: &[[f64; 6]], face: Face) -> Vec<[f64; 4]> {
    let (axis, plane) = match face {
        Face::Down => (1, FACE_EPSILON),
        Face::Up => (1, 1.0 - FACE_EPSILON),
        Face::North => (2, FACE_EPSILON),
        Face::South => (2, 1.0 - FACE_EPSILON),
        Face::West => (0, FACE_EPSILON),
        Face::East => (0, 1.0 - FACE_EPSILON),
    };
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    boxes
        .iter()
        .filter(|b| b[axis] < plane && b[axis + 3] > plane)
        .map(|b| [b[u], b[v], b[u + 3], b[v + 3]])
        .collect()
}

/// Whether `boxes` cover the rectangle `area` of `face` entirely, as vanilla's tests of
/// a face against a shape (`Shapes.joinIsNotEmpty(test, face, ONLY_FIRST)`) find.
pub fn face_covers(boxes: &[[f64; 6]], face: Face, area: [f64; 4]) -> bool {
    let rectangles = face_rectangles(boxes, face);
    // The rectangles' edges divide the area into cells, each wholly in or out of every
    // rectangle; the area is covered if the middle of every cell is.
    let mut us = vec![area[0], area[2]];
    let mut vs = vec![area[1], area[3]];
    for r in &rectangles {
        for u in [r[0], r[2]] {
            if u > area[0] && u < area[2] {
                us.push(u);
            }
        }
        for v in [r[1], r[3]] {
            if v > area[1] && v < area[3] {
                vs.push(v);
            }
        }
    }
    us.sort_by(f64::total_cmp);
    vs.sort_by(f64::total_cmp);
    us.dedup();
    vs.dedup();
    us.windows(2).all(|u| {
        vs.windows(2).all(|v| {
            let middle = [(u[0] + u[1]) / 2.0, (v[0] + v[1]) / 2.0];
            rectangles.iter().any(|r| {
                r[0] <= middle[0] && middle[0] <= r[2] && r[1] <= middle[1] && middle[1] <= r[3]
            })
        })
    })
}

/// Vanilla's `Block.isFaceFull`: whether `boxes` cover the whole of `face`.
pub fn face_full(boxes: &[[f64; 6]], face: Face) -> bool {
    face_covers(boxes, face, [0.0, 0.0, 1.0, 1.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_slab_covers_its_bottom_and_not_its_top() {
        let slab = [[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
        assert!(face_full(&slab, Face::Down));
        assert!(!face_full(&slab, Face::Up));
        assert!(!face_full(&slab, Face::North));
        assert!(face_covers(&slab, Face::North, [0.0, 0.0, 1.0, 0.5]));
    }

    #[test]
    fn two_halves_side_by_side_cover_a_face_together() {
        let halves = [
            [0.0, 0.0, 0.0, 0.5, 1.0, 1.0],
            [0.5, 0.0, 0.0, 1.0, 1.0, 1.0],
        ];
        assert!(face_full(&halves, Face::Up));
        assert!(!face_full(&halves[..1], Face::Up));
    }
}
