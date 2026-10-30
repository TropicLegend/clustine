//! The frame of a Nether portal from the game's rules (`docs/groundwork/p-brief.md`,
//! sections 4.1 and 4.2; PortalShape.txt and NetherPortalBlock.txt 103–148), written
//! by someone who did not write the search: the largest frame and one block over, a
//! frame with one block of a side missing, a frame with a block inside, frames of both
//! axes sharing a post, lighting from every cell of the inside, the corners, and when
//! a portal block stays.

use std::collections::BTreeMap;

use clustine_data::{BlockState, Face, blocks};
use clustine_world::{BlockPos, PortalAxis};

use super::{PortalShape, find_any_shape, find_empty_portal_shape, portal_stays};
use crate::data::set;

/// The overworld's floor, far below every frame here.
const MIN_Y: i32 = -64;

/// A world of air but for the blocks set.
#[derive(Default, Clone)]
struct World(BTreeMap<BlockPos, BlockState>);

impl World {
    fn set(&mut self, pos: BlockPos, state: BlockState) {
        if state == blocks::AIR {
            self.0.remove(&pos);
        } else {
            self.0.insert(pos, state);
        }
    }

    fn get(&self, pos: BlockPos) -> BlockState {
        self.0.get(&pos).copied().unwrap_or(blocks::AIR)
    }

    fn empty_shape(&self, pos: BlockPos, axis: PortalAxis) -> Option<PortalShape> {
        let mut read = |at: BlockPos| self.get(at);
        find_empty_portal_shape(&mut read, MIN_Y, pos, axis)
    }

    fn any_shape(&self, pos: BlockPos, axis: PortalAxis) -> PortalShape {
        let mut read = |at: BlockPos| self.get(at);
        find_any_shape(&mut read, MIN_Y, pos, axis)
    }

    fn stays(&self, pos: BlockPos, direction: Face, neighbour: BlockState) -> bool {
        let state = self.get(pos);
        let mut read = |at: BlockPos| self.get(at);
        portal_stays(&mut read, MIN_Y, state, pos, direction, neighbour)
    }
}

/// A frame of obsidian whose inside is `width` by `height`, its lowest inside block
/// with the least coordinate along the axis at `base`. `u` counts along the axis (x
/// for X, z for Z) and `v` up.
#[derive(Clone, Copy)]
struct Frame {
    axis: PortalAxis,
    base: BlockPos,
    width: i32,
    height: i32,
}

impl Frame {
    fn new(axis: PortalAxis, base: BlockPos, width: i32, height: i32) -> Self {
        Self {
            axis,
            base,
            width,
            height,
        }
    }

    /// The block `u` along the axis and `v` up from the base.
    fn at(&self, u: i32, v: i32) -> BlockPos {
        match self.axis {
            PortalAxis::X => BlockPos::new(self.base.x + u, self.base.y + v, self.base.z),
            PortalAxis::Z => BlockPos::new(self.base.x, self.base.y + v, self.base.z + u),
        }
    }

    fn inside(&self) -> Vec<BlockPos> {
        let mut cells = Vec::new();
        for v in 0..self.height {
            for u in 0..self.width {
                cells.push(self.at(u, v));
            }
        }
        cells
    }

    /// The frame's blocks without its four corners.
    fn sides(&self) -> Vec<BlockPos> {
        let mut blocks = Vec::new();
        for u in 0..self.width {
            blocks.push(self.at(u, -1));
            blocks.push(self.at(u, self.height));
        }
        for v in 0..self.height {
            blocks.push(self.at(-1, v));
            blocks.push(self.at(self.width, v));
        }
        blocks
    }

    fn corners(&self) -> [BlockPos; 4] {
        [
            self.at(-1, -1),
            self.at(self.width, -1),
            self.at(-1, self.height),
            self.at(self.width, self.height),
        ]
    }

    /// The bottom left as the game finds it: the right is west along X and south
    /// along Z, so the left end of the bottom row is the east one along X (the
    /// greatest x) and the north one along Z (the least z).
    fn bottom_left(&self) -> BlockPos {
        match self.axis {
            PortalAxis::X => self.at(self.width - 1, 0),
            PortalAxis::Z => self.at(0, 0),
        }
    }

    fn build(&self, world: &mut World, corners: bool) {
        for pos in self.sides() {
            world.set(pos, blocks::OBSIDIAN);
        }
        if corners {
            for pos in self.corners() {
                world.set(pos, blocks::OBSIDIAN);
            }
        }
    }

    fn world(&self) -> World {
        let mut world = World::default();
        self.build(&mut world, true);
        world
    }

    fn fill(&self, world: &mut World) {
        let state = set(blocks::NETHER_PORTAL, "axis", axis_name(self.axis));
        for pos in self.inside() {
            world.set(pos, state);
        }
    }

    fn expected(&self, portal_blocks: i32) -> PortalShape {
        PortalShape {
            axis: self.axis,
            portal_blocks,
            bottom_left: self.bottom_left(),
            width: self.width,
            height: self.height,
        }
    }
}

fn axis_name(axis: PortalAxis) -> &'static str {
    match axis {
        PortalAxis::X => "x",
        PortalAxis::Z => "z",
    }
}

fn other(axis: PortalAxis) -> PortalAxis {
    match axis {
        PortalAxis::X => PortalAxis::Z,
        PortalAxis::Z => PortalAxis::X,
    }
}

const AXES: [PortalAxis; 2] = [PortalAxis::X, PortalAxis::Z];

fn base() -> BlockPos {
    BlockPos::new(3, 70, -5)
}

/// The largest frame, 21 by 21 inside, is found from every block of its inside,
/// along either axis asked first, with its bottom left where the game puts it.
#[test]
fn the_largest_frame_is_found_from_every_block_of_its_inside() {
    for axis in AXES {
        let frame = Frame::new(axis, base(), 21, 21);
        let world = frame.world();
        for pos in frame.inside() {
            for asked in [axis, other(axis)] {
                assert_eq!(
                    world.empty_shape(pos, asked),
                    Some(frame.expected(0)),
                    "{axis:?} asked {asked:?} at {pos:?}"
                );
            }
        }
    }
}

/// One block wider than the largest, or one taller, is no frame, wherever it is
/// lit; and so is a frame one narrower or lower than the smallest.
#[test]
fn a_frame_one_block_over_the_largest_or_under_the_smallest_is_no_frame() {
    for axis in AXES {
        for (width, height) in [(22, 21), (21, 22), (22, 22), (1, 3), (2, 2), (1, 21)] {
            let frame = Frame::new(axis, base(), width, height);
            let world = frame.world();
            for pos in frame.inside() {
                assert_eq!(
                    world.empty_shape(pos, axis),
                    None,
                    "{axis:?} {width} by {height} at {pos:?}"
                );
            }
        }
        // The smallest is a frame.
        let frame = Frame::new(axis, base(), 2, 3);
        let world = frame.world();
        for pos in frame.inside() {
            assert_eq!(world.empty_shape(pos, axis), Some(frame.expected(0)));
        }
    }
}

/// Lit from the top row of its inside, a frame 21 high is found: the search goes down
/// at most 21 blocks, and 20 bring it to the bottom row. In the middle and in each
/// corner of the inside too.
#[test]
fn a_frame_is_found_from_its_top_row_its_middle_and_each_corner_of_its_inside() {
    for axis in AXES {
        for (width, height) in [(21, 21), (5, 7), (2, 21), (21, 3)] {
            let frame = Frame::new(axis, base(), width, height);
            let world = frame.world();
            let points = [
                frame.at(0, height - 1),
                frame.at(width - 1, height - 1),
                frame.at(width / 2, height - 1),
                frame.at(width / 2, height / 2),
                frame.at(0, 0),
                frame.at(width - 1, 0),
            ];
            for pos in points {
                assert_eq!(
                    world.empty_shape(pos, PortalAxis::X),
                    Some(frame.expected(0)),
                    "{axis:?} {width} by {height} at {pos:?}"
                );
            }
        }
    }
}

/// A frame with one block of a side missing, any one but a corner, is no frame from
/// anywhere inside: not the bottom (each block of it must have frame below the row
/// above), not a post (each row must have frame at both ends) and not the top.
#[test]
fn a_frame_missing_one_block_of_any_side_is_no_frame() {
    for axis in AXES {
        let frame = Frame::new(axis, base(), 4, 5);
        let mut missing = Vec::new();
        // The middle of each side and the ends of each side next to a corner.
        for u in [0, 1, 3] {
            missing.push(frame.at(u, -1));
            missing.push(frame.at(u, 5));
        }
        for v in [0, 2, 4] {
            missing.push(frame.at(-1, v));
            missing.push(frame.at(4, v));
        }
        for gap in missing {
            let mut world = frame.world();
            world.set(gap, blocks::AIR);
            for pos in frame.inside() {
                assert_eq!(
                    world.empty_shape(pos, axis),
                    None,
                    "{axis:?} without {gap:?}, lit at {pos:?}"
                );
            }
        }
    }
}

/// The corners are never looked at: a frame without any of them is found as one with
/// all four.
#[test]
fn a_frame_without_its_corners_is_a_frame() {
    for axis in AXES {
        let frame = Frame::new(axis, base(), 3, 4);
        let mut world = World::default();
        frame.build(&mut world, false);
        for pos in frame.inside() {
            assert_eq!(world.empty_shape(pos, axis), Some(frame.expected(0)));
        }
        // A corner of another block is no corner either.
        let mut world = frame.world();
        for corner in frame.corners() {
            world.set(corner, blocks::STONE);
        }
        assert_eq!(
            world.empty_shape(frame.at(1, 1), axis),
            Some(frame.expected(0))
        );
    }
}

/// One solid block anywhere inside, and the frame is found from nowhere inside it.
#[test]
fn a_frame_whose_inside_holds_one_solid_block_is_no_frame() {
    for axis in AXES {
        let frame = Frame::new(axis, base(), 3, 4);
        for solid in frame.inside() {
            let mut world = frame.world();
            world.set(solid, blocks::STONE);
            for pos in frame.inside().into_iter().filter(|pos| *pos != solid) {
                assert_eq!(
                    world.empty_shape(pos, axis),
                    None,
                    "{axis:?} with stone at {solid:?}, lit at {pos:?}"
                );
            }
        }
    }
}

/// Fire and soul fire inside are empty to the search, and so is portal; a frame that
/// holds some portal is found, but not as an empty one.
#[test]
fn fire_inside_is_empty_and_portal_inside_is_counted() {
    for axis in AXES {
        let frame = Frame::new(axis, base(), 3, 4);
        let mut world = frame.world();
        world.set(frame.at(0, 0), blocks::FIRE);
        world.set(frame.at(2, 3), blocks::SOUL_FIRE);
        assert_eq!(
            world.empty_shape(frame.at(1, 1), axis),
            Some(frame.expected(0))
        );
        let portal = set(blocks::NETHER_PORTAL, "axis", axis_name(axis));
        world.set(frame.at(1, 2), portal);
        assert_eq!(world.empty_shape(frame.at(1, 1), axis), None);
        assert_eq!(world.any_shape(frame.at(1, 1), axis), frame.expected(1));
        assert!(!world.any_shape(frame.at(1, 1), axis).is_complete());
        frame.fill(&mut world);
        let full = world.any_shape(frame.at(2, 3), axis);
        assert_eq!(full, frame.expected(12));
        assert!(full.is_complete());
    }
}

/// Two frames of both axes that share a post: from a block of the one along X the
/// frame along X is found whichever axis is asked first, and from the other's the
/// frame along Z.
#[test]
fn frames_of_both_axes_sharing_a_post_are_each_found_from_their_own_inside() {
    // Along X: inside x = 1..=2 at z = 0; its east post, x = 3, is also the north post
    // of the frame along Z, whose inside is z = 1..=2 at x = 3.
    let along_x = Frame::new(PortalAxis::X, BlockPos::new(1, 70, 0), 2, 3);
    let along_z = Frame::new(PortalAxis::Z, BlockPos::new(3, 70, 1), 2, 3);
    let mut world = World::default();
    along_x.build(&mut world, true);
    along_z.build(&mut world, true);
    assert_eq!(world.get(BlockPos::new(3, 71, 0)), blocks::OBSIDIAN);
    for asked in AXES {
        for pos in along_x.inside() {
            assert_eq!(world.empty_shape(pos, asked), Some(along_x.expected(0)));
        }
        for pos in along_z.inside() {
            assert_eq!(world.empty_shape(pos, asked), Some(along_z.expected(0)));
        }
    }
}

/// Where a block is the inside of a frame along X and of one along Z, the axis asked
/// first wins; and once one is full of portal the other holds portal and is no empty
/// frame.
#[test]
fn a_block_inside_frames_of_both_axes_takes_the_axis_asked_first() {
    // Along X: inside x = 1..=2 at z = 0. Along Z: inside z = 0..=1 at x = 1. They
    // share the column x = 1, z = 0.
    let along_x = Frame::new(PortalAxis::X, BlockPos::new(1, 70, 0), 2, 3);
    let along_z = Frame::new(PortalAxis::Z, BlockPos::new(1, 70, 0), 2, 3);
    let mut world = World::default();
    along_x.build(&mut world, true);
    along_z.build(&mut world, true);
    // Neither frame's posts stand inside the other: x = 0 and x = 3 at z = 0 for X,
    // z = -1 and z = 2 at x = 1 for Z.
    let shared = BlockPos::new(1, 71, 0);
    assert_eq!(
        world.empty_shape(shared, PortalAxis::X),
        Some(along_x.expected(0))
    );
    assert_eq!(
        world.empty_shape(shared, PortalAxis::Z),
        Some(along_z.expected(0))
    );
    along_x.fill(&mut world);
    // The frame along Z now holds three portal blocks.
    let only_z = BlockPos::new(1, 71, 1);
    assert_eq!(world.empty_shape(only_z, PortalAxis::Z), None);
    assert_eq!(world.empty_shape(only_z, PortalAxis::X), None);
    assert_eq!(world.any_shape(only_z, PortalAxis::Z), along_z.expected(3));
}

/// A frame floating in the air is found from the corner of its inside on its bottom.
#[test]
fn a_frame_floating_in_the_air_is_found_from_its_bottom_corner() {
    for axis in AXES {
        let frame = Frame::new(axis, BlockPos::new(-8, 150, 9), 4, 5);
        let world = frame.world();
        for pos in [frame.at(0, 0), frame.at(3, 0)] {
            assert_eq!(world.empty_shape(pos, axis), Some(frame.expected(0)));
        }
        // Below the frame, in the air, there is none.
        assert_eq!(world.empty_shape(frame.at(1, -2), axis), None);
    }
}

/// The cells set are the inside, in the order of `BlockPos.betweenClosed` over the
/// box: x fastest, then y, then z.
#[test]
fn the_cells_are_the_inside_x_fastest_then_y_then_z() {
    let along_x = Frame::new(PortalAxis::X, BlockPos::new(1, 70, 0), 2, 3).expected(0);
    let along_x: Vec<(i32, i32, i32)> = along_x.cells().iter().map(|p| (p.x, p.y, p.z)).collect();
    assert_eq!(
        along_x,
        [
            (1, 70, 0),
            (2, 70, 0),
            (1, 71, 0),
            (2, 71, 0),
            (1, 72, 0),
            (2, 72, 0)
        ]
    );
    let along_z = Frame::new(PortalAxis::Z, BlockPos::new(1, 70, 0), 2, 3).expected(0);
    let along_z: Vec<(i32, i32, i32)> = along_z.cells().iter().map(|p| (p.x, p.y, p.z)).collect();
    assert_eq!(
        along_z,
        [
            (1, 70, 0),
            (1, 71, 0),
            (1, 72, 0),
            (1, 70, 1),
            (1, 71, 1),
            (1, 72, 1)
        ]
    );
}

/// A portal block stays where its neighbour in front or behind changes, whatever the
/// frame; where the neighbour is portal of either axis; and where its frame is whole
/// and full. Else it goes.
#[test]
fn a_portal_block_stays_only_by_the_rules_of_update_shape() {
    for axis in AXES {
        let frame = Frame::new(axis, base(), 2, 3);
        let mut world = frame.world();
        frame.fill(&mut world);
        let pos = frame.at(0, 1);
        let (front, sideways) = match axis {
            PortalAxis::X => ([Face::North, Face::South], [Face::East, Face::West]),
            PortalAxis::Z => ([Face::East, Face::West], [Face::North, Face::South]),
        };
        // Whole and full: every change is borne.
        for face in Face::ALL {
            assert!(world.stays(pos, face, blocks::STONE), "{axis:?} {face:?}");
        }
        // A post block gone.
        let mut broken = world.clone();
        broken.set(frame.at(-1, 1), blocks::AIR);
        for face in front {
            assert!(broken.stays(pos, face, blocks::AIR), "{axis:?} {face:?}");
        }
        for face in sideways.into_iter().chain([Face::Up, Face::Down]) {
            assert!(!broken.stays(pos, face, blocks::AIR), "{axis:?} {face:?}");
            for portal_axis in ["x", "z"] {
                let portal = set(blocks::NETHER_PORTAL, "axis", portal_axis);
                assert!(broken.stays(pos, face, portal), "{axis:?} {face:?}");
            }
        }
        // A portal block gone from inside.
        let mut holed = world.clone();
        holed.set(frame.at(1, 2), blocks::AIR);
        assert!(!holed.stays(pos, Face::Up, blocks::AIR));
        // The corners do not matter.
        let mut cornerless = world.clone();
        for corner in frame.corners() {
            cornerless.set(corner, blocks::AIR);
        }
        assert!(cornerless.stays(pos, Face::Down, blocks::AIR));
    }
}
