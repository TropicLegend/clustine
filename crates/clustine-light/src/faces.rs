//! When the two faces between two blocks stop light.
//!
//! Adapted from SteelMC's `steel-core/src/chunk/light/mod.rs` (`light_occlusion_shape`,
//! `light_face_occludes`) and `steel-core/src/physics/shapes.rs` (`face_shape_occludes`),
//! branch 26.3 at commit 885c4b3e60ed79862c37311780774f76806cb714, AGPL-3.0. The rule is
//! theirs, which is the game's `LightEngine.shapeOccludes`; the test of whether
//! rectangles cover a face is written anew for the rectangles of
//! `clustine-data`'s table.

use clustine_data::{BlockState, Face, FaceShape};

/// The game compares the corners of shapes with this much to spare.
const EPSILON: f64 = 1.0e-7;

/// What `state` covers of `face` against light: its occlusion face if it occludes and
/// does so by its shape, nothing otherwise. A block that stops light altogether does it
/// by its dampening and has nothing here.
pub(crate) fn occlusion(state: BlockState, face: Face) -> FaceShape {
    let props = state.props();
    if props.occludes() && props.occludes_by_shape() {
        props.occlusion_face(face)
    } else {
        FaceShape::EMPTY
    }
}

/// Whether `state` can stop light on some face by its shape.
pub(crate) fn has_shape(state: BlockState) -> bool {
    let props = state.props();
    props.occludes() && props.occludes_by_shape() && props.occlusion_face_set() != 0
}

/// Whether light is stopped on its way out of `from` through `towards` into `to`: one of
/// the two faces that meet there is whole, or the two together cover the face.
pub(crate) fn stop_light(from: BlockState, to: BlockState, towards: Face) -> bool {
    let near = occlusion(from, towards);
    let far = occlusion(to, towards.opposite());
    if near.is_full() || far.is_full() {
        return true;
    }
    if near.is_empty() && far.is_empty() {
        return false;
    }
    covered([0.0, 0.0, 1.0, 1.0], near, far, 0)
}

/// Whether `area` (`[min u, min v, max u, max v]`) lies within the union of the
/// rectangles of the two shapes, of which the first `skip` are known not to touch it.
///
/// The first rectangle that overlaps the area is cut out of it, and what is left of the
/// area, at most four smaller ones, has to be covered by the rectangles after that one.
fn covered(area: [f64; 4], near: FaceShape, far: FaceShape, skip: usize) -> bool {
    let [min_u, min_v, max_u, max_v] = area;
    if max_u - min_u <= EPSILON || max_v - min_v <= EPSILON {
        return true;
    }
    let rectangles = near.rectangles().chain(far.rectangles());
    for (index, [low_u, low_v, high_u, high_v]) in rectangles.enumerate().skip(skip) {
        let overlaps = low_u < max_u - EPSILON
            && high_u > min_u + EPSILON
            && low_v < max_v - EPSILON
            && high_v > min_v + EPSILON;
        if !overlaps {
            continue;
        }
        let next = index + 1;
        let from_u = low_u.max(min_u);
        let to_u = high_u.min(max_u);
        return covered([min_u, min_v, from_u, max_v], near, far, next)
            && covered([to_u, min_v, max_u, max_v], near, far, next)
            && covered([from_u, min_v, to_u, low_v.max(min_v)], near, far, next)
            && covered([from_u, high_v.min(max_v), to_u, max_v], near, far, next);
    }
    false
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;

    use super::*;

    fn state(text: &str) -> BlockState {
        BlockState::parse(text).unwrap()
    }

    #[test]
    fn blocks_without_a_shape_for_light_cover_nothing() {
        for plain in [blocks::AIR, blocks::STONE, blocks::GLASS, blocks::WATER] {
            assert!(!has_shape(plain));
            for face in Face::ALL {
                assert!(occlusion(plain, face).is_empty());
                assert!(!stop_light(plain, blocks::AIR, face));
            }
        }
    }

    #[test]
    fn a_bottom_slab_covers_its_underside_and_half_of_each_side() {
        let slab = state("minecraft:oak_slab[type=bottom]");
        assert!(has_shape(slab));
        assert!(occlusion(slab, Face::Down).is_full());
        assert!(occlusion(slab, Face::Up).is_empty());
        assert!(stop_light(slab, blocks::AIR, Face::Down));
        assert!(stop_light(blocks::AIR, slab, Face::Up));
        assert!(!stop_light(slab, blocks::AIR, Face::Up));
        for side in [Face::North, Face::South, Face::West, Face::East] {
            assert!(!occlusion(slab, side).is_empty());
            assert!(!occlusion(slab, side).is_full());
            assert!(!stop_light(slab, blocks::AIR, side));
            // Two lower halves leave the upper half of the face open.
            assert!(!stop_light(slab, slab, side));
        }
    }

    #[test]
    fn a_lower_and_an_upper_half_cover_a_side_together() {
        let bottom = state("minecraft:oak_slab[type=bottom]");
        let top = state("minecraft:oak_slab[type=top]");
        for side in [Face::North, Face::South, Face::West, Face::East] {
            assert!(stop_light(bottom, top, side));
            assert!(stop_light(top, bottom, side));
        }
    }

    #[test]
    fn a_stair_is_whole_at_its_back_and_its_base() {
        let stair = state("minecraft:oak_stairs[facing=east,half=bottom,shape=straight]");
        assert!(occlusion(stair, Face::East).is_full());
        assert!(occlusion(stair, Face::Down).is_full());
        assert!(!occlusion(stair, Face::West).is_full());
        assert!(stop_light(stair, blocks::AIR, Face::East));
        assert!(!stop_light(stair, blocks::AIR, Face::West));
        // The step's side is an L: a slab's lower half does not fill it, another stair
        // turned the other way and upside down does.
        let bottom = state("minecraft:oak_slab[type=bottom]");
        assert!(!stop_light(stair, bottom, Face::North));
        let other = state("minecraft:oak_stairs[facing=west,half=top,shape=straight]");
        assert!(stop_light(stair, other, Face::North));
    }
}
