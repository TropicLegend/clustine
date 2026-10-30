//! Tests of a Nether portal's travel from the game's rules (`docs/groundwork/p-brief.md`,
//! sections 2.2 and 2.3; PortalForcer.txt, NetherPortalBlock.txt, PortalShape.txt,
//! BlockUtil.txt, CollisionGetter.txt), with the arithmetic worked from the bytecode.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, PortalAxis};

use super::*;
use crate::data::set;

/// A player standing: `EntityDimensions` of 0.6 by 1.8, as floats.
const PLAYER: (f32, f32) = (0.6, 1.8);

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0e-6
}

fn assert_close(actual: [f64; 3], expected: [f64; 3]) {
    assert!(
        (0..3).all(|axis| close(actual[axis], expected[axis])),
        "{actual:?} is not {expected:?}"
    );
}

fn portal(axis: PortalAxis) -> BlockState {
    set(
        blocks::NETHER_PORTAL,
        "axis",
        crate::portal::axis_name(axis),
    )
}

/// A world: a ground of `ground` (stone up to a height by column, none for no ground)
/// and blocks put over it; air elsewhere, and positions the caller cannot read.
struct World {
    ground: Box<dyn Fn(i32, i32) -> Option<i32>>,
    fill: BlockState,
    blocks: BTreeMap<BlockPos, BlockState>,
    unreadable: BTreeSet<BlockPos>,
    min_y: i32,
}

impl World {
    fn new(ground: impl Fn(i32, i32) -> Option<i32> + 'static) -> Self {
        World {
            ground: Box::new(ground),
            fill: blocks::STONE,
            blocks: BTreeMap::new(),
            unreadable: BTreeSet::new(),
            min_y: -64,
        }
    }

    fn empty() -> Self {
        World::new(|_, _| None)
    }

    fn put(&mut self, pos: BlockPos, state: BlockState) {
        self.blocks.insert(pos, state);
    }

    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if self.unreadable.contains(&pos) {
            return None;
        }
        if let Some(state) = self.blocks.get(&pos) {
            return Some(*state);
        }
        match (self.ground)(pos.x, pos.z) {
            Some(top) if pos.y <= top && pos.y >= self.min_y => Some(self.fill),
            _ => Some(blocks::AIR),
        }
    }

    /// The heightmap of the ground alone, which is all these tests need of it.
    fn height(&self, x: i32, z: i32) -> Option<i32> {
        Some(match (self.ground)(x, z) {
            Some(top) => top + 1,
            None => self.min_y,
        })
    }

    fn make(&self, target: BlockPos, axis: PortalAxis, logical_height: i32) -> Option<MadePortal> {
        self.make_within(target, axis, logical_height, DEFAULT_BORDER)
    }

    fn make_within(
        &self,
        target: BlockPos,
        axis: PortalAxis,
        logical_height: i32,
        border: f64,
    ) -> Option<MadePortal> {
        make_portal(
            target,
            axis,
            &|pos| self.block(pos),
            &|x, z| self.height(x, z),
            self.min_y,
            logical_height,
            border,
        )
    }

    /// Puts a frame of obsidian about a portal along `axis` whose lowest block on the
    /// negative side is `corner`, `width` by `height`.
    fn put_portal(&mut self, corner: BlockPos, axis: PortalAxis, width: i32, height: i32) {
        let step = |pos: BlockPos, along: i32, up: i32| match axis {
            PortalAxis::X => at(pos.x + along, pos.y + up, pos.z),
            PortalAxis::Z => at(pos.x, pos.y + up, pos.z + along),
        };
        for along in -1..=width {
            for up in -1..=height {
                let inside = (0..width).contains(&along) && (0..height).contains(&up);
                let state = if inside {
                    portal(axis)
                } else {
                    blocks::OBSIDIAN
                };
                self.put(step(corner, along, up), state);
            }
        }
    }
}

/// The blocks `createPortal` sets for a frame about `spot` along `axis`, then the portal
/// blocks inside it, in the game's order.
fn frame_and_portal(spot: BlockPos, axis: PortalAxis) -> Vec<(BlockPos, BlockState)> {
    let step = |along: i32, up: i32| match axis {
        PortalAxis::X => at(spot.x + along, spot.y + up, spot.z),
        PortalAxis::Z => at(spot.x, spot.y + up, spot.z + along),
    };
    let mut changes = Vec::new();
    for along in -1..=2 {
        for up in -1..=3 {
            if along == -1 || along == 2 || up == -1 || up == 3 {
                changes.push((step(along, up), blocks::OBSIDIAN));
            }
        }
    }
    for along in 0..=1 {
        for up in 0..=2 {
            changes.push((step(along, up), portal(axis)));
        }
    }
    changes
}

// nearest_portal

#[test]
fn a_portal_two_blocks_away_is_chosen_over_one_three_away() {
    let target = at(0, 64, 0);
    let found = nearest_portal(&[at(3, 64, 0), at(0, 64, 2)], target, 16, DEFAULT_BORDER);
    assert_eq!(found, Some(at(0, 64, 2)));
}

#[test]
fn of_two_portals_as_near_the_lower_is_chosen() {
    // Both are 4 away by `distSqr`: 2 above and 2 below.
    let target = at(5, 64, 5);
    let found = nearest_portal(&[at(5, 66, 5), at(5, 62, 5)], target, 16, DEFAULT_BORDER);
    assert_eq!(found, Some(at(5, 62, 5)));
}

#[test]
fn of_two_portals_as_near_and_as_high_the_first_found_is_chosen() {
    let target = at(0, 64, 0);
    let found = nearest_portal(&[at(0, 64, 1), at(1, 64, 0)], target, 16, DEFAULT_BORDER);
    assert_eq!(found, Some(at(0, 64, 1)));
}

#[test]
fn a_portal_is_looked_for_in_a_square_and_not_a_circle() {
    let target = at(0, 64, 0);
    // A corner of the square is farther than a block just outside it, which is not
    // looked at.
    let found = nearest_portal(&[at(17, 64, 0), at(16, 64, 16)], target, 16, DEFAULT_BORDER);
    assert_eq!(found, Some(at(16, 64, 16)));
    assert_eq!(
        nearest_portal(
            &[at(0, 64, -17)],
            target,
            NETHER_PORTAL_RADIUS,
            DEFAULT_BORDER
        ),
        None
    );
    assert_eq!(
        nearest_portal(
            &[at(0, 64, -17)],
            target,
            OVERWORLD_PORTAL_RADIUS,
            DEFAULT_BORDER
        ),
        Some(at(0, 64, -17))
    );
}

#[test]
fn a_portal_beyond_the_border_is_not_chosen() {
    let target = at(9, 64, 0);
    // The border's high edge is out: a block at x = 10 is beyond a border of 10.
    let found = nearest_portal(&[at(10, 64, 0), at(5, 64, 0)], target, 16, 10.0);
    assert_eq!(found, Some(at(5, 64, 0)));
    assert_eq!(nearest_portal(&[], target, 16, 10.0), None);
}

// largest_rectangle

#[test]
fn the_rectangle_of_a_two_by_three_portal_is_found_from_any_of_its_blocks() {
    for axis in [PortalAxis::X, PortalAxis::Z] {
        let mut world = World::empty();
        let corner = at(4, 64, -7);
        world.put_portal(corner, axis, 2, 3);
        let read = |pos| world.block(pos);
        for along in 0..2 {
            for up in 0..3 {
                let block = match axis {
                    PortalAxis::X => at(corner.x + along, corner.y + up, corner.z),
                    PortalAxis::Z => at(corner.x, corner.y + up, corner.z + along),
                };
                assert_eq!(
                    largest_rectangle(block, axis, &read),
                    FoundRectangle {
                        min_corner: corner,
                        axis1_size: 2,
                        axis2_size: 3,
                    },
                    "from {block:?} along {axis:?}"
                );
            }
        }
    }
}

#[test]
fn the_rectangle_of_a_twenty_one_by_twenty_one_portal_is_found_whole() {
    let mut world = World::empty();
    let corner = at(-10, 70, 3);
    world.put_portal(corner, PortalAxis::X, 21, 21);
    let read = |pos| world.block(pos);
    let whole = FoundRectangle {
        min_corner: corner,
        axis1_size: 21,
        axis2_size: 21,
    };
    for block in [at(0, 80, 3), at(-10, 70, 3), at(10, 90, 3), at(-10, 90, 3)] {
        assert_eq!(largest_rectangle(block, PortalAxis::X, &read), whole);
    }
}

#[test]
fn the_rectangle_of_an_uneven_patch_is_the_largest_that_reaches_the_block() {
    // A patch of portal along X: columns x = 0..4, the first two 3 high and the rest
    // 5 high, from y = 64. From (3, 64) the 3 × 5 on the right and the 5 × 3 along
    // the bottom are both 15; the histogram's stack meets the right part first and
    // keeps it, since only a larger one replaces it.
    let mut world = World::empty();
    let state = portal(PortalAxis::X);
    for x in 0..5 {
        let height = if x < 2 { 3 } else { 5 };
        for y in 64..64 + height {
            world.put(at(x, y, 0), state);
        }
    }
    let read = |pos| world.block(pos);
    assert_eq!(
        largest_rectangle(at(3, 64, 0), PortalAxis::X, &read),
        FoundRectangle {
            min_corner: at(2, 64, 0),
            axis1_size: 3,
            axis2_size: 5,
        }
    );
    // From the left part the columns to the right are limited to its own reach up,
    // so the whole bottom 5 × 3 is the largest.
    assert_eq!(
        largest_rectangle(at(0, 64, 0), PortalAxis::X, &read),
        FoundRectangle {
            min_corner: at(0, 64, 0),
            axis1_size: 5,
            axis2_size: 3,
        }
    );
}

#[test]
fn a_portal_block_that_cannot_be_read_has_a_rectangle_of_itself() {
    let read = |_| None;
    assert_eq!(
        largest_rectangle(at(1, 2, 3), PortalAxis::Z, &read),
        FoundRectangle {
            min_corner: at(1, 2, 3),
            axis1_size: 1,
            axis2_size: 1,
        }
    );
}

// relative_position and exit_relative

fn two_by_three(corner: BlockPos) -> FoundRectangle {
    FoundRectangle {
        min_corner: corner,
        axis1_size: 2,
        axis2_size: 3,
    }
}

#[test]
fn a_player_in_the_middle_of_a_portal_is_half_way_along_it() {
    let rectangle = two_by_three(at(0, 64, 0));
    // Along: (1 − 0.3) / (2 − 0.6) = 0.5; up: 0 at the bottom; across: 0 in the
    // middle plane.
    let relative = relative_position(&rectangle, PortalAxis::X, [1.0, 64.0, 0.5], PLAYER);
    assert_close(relative, [0.5, 0.0, 0.0]);
    // Up: (64.6 − 64) / (3 − 1.8) = 0.5; across: 0.8 − 0.5.
    let relative = relative_position(&rectangle, PortalAxis::X, [1.0, 64.6, 0.8], PLAYER);
    assert_close(relative, [0.5, 0.5, 0.3]);
}

#[test]
fn along_a_z_portal_the_position_is_read_on_z_and_across_on_x() {
    let rectangle = two_by_three(at(5, 64, -3));
    let relative = relative_position(&rectangle, PortalAxis::Z, [5.7, 64.0, -2.0], PLAYER);
    assert_close(relative, [0.5, 0.0, 0.2]);
}

#[test]
fn a_player_at_a_portals_edge_or_outside_it_is_held_within_it_along_and_up() {
    let rectangle = two_by_three(at(0, 64, 0));
    // As far left as a player fits: 0.3 is the start of the room.
    let relative = relative_position(&rectangle, PortalAxis::X, [0.3, 64.0, 0.5], PLAYER);
    assert_close(relative, [0.0, 0.0, 0.0]);
    // As far right as fits: (1.7 − 0.3) / 1.4.
    let relative = relative_position(&rectangle, PortalAxis::X, [1.7, 64.0, 0.5], PLAYER);
    assert_close(relative, [1.0, 0.0, 0.0]);
    // Outside: held at 0 and 1 along and up, and across not held at all.
    let relative = relative_position(&rectangle, PortalAxis::X, [-5.0, 100.0, 3.0], PLAYER);
    assert_close(relative, [0.0, 1.0, 2.5]);
    let relative = relative_position(&rectangle, PortalAxis::X, [10.0, 0.0, -3.0], PLAYER);
    assert_close(relative, [1.0, 0.0, -3.5]);
}

#[test]
fn an_entity_wider_or_taller_than_the_portal_is_put_in_its_middle_and_at_its_bottom() {
    let rectangle = two_by_three(at(0, 64, 0));
    let relative = relative_position(&rectangle, PortalAxis::X, [1.9, 65.0, 0.5], (2.5, 3.5));
    assert_close(relative, [0.5, 0.0, 0.0]);
    // A width of exactly 2 leaves no room either.
    let relative = relative_position(&rectangle, PortalAxis::X, [1.9, 65.0, 0.5], (2.0, 3.0));
    assert_close(relative, [0.5, 0.0, 0.0]);
}

#[test]
fn leaving_by_a_portal_block_measures_its_rectangle_along_its_axis() {
    let mut world = World::empty();
    world.put_portal(at(10, 64, 10), PortalAxis::Z, 3, 4);
    let read = |pos| world.block(pos);
    // Room along: 3 − 0.6; at z = 11.5 the player is (11.5 − 10.3) / 2.4 = 0.5 along.
    let (axis, relative) = exit_relative(at(10, 65, 11), [10.5, 64.0, 11.5], PLAYER, &read);
    assert_eq!(axis, PortalAxis::Z);
    assert_close(relative, [0.5, 0.0, 0.0]);
}

#[test]
fn leaving_by_a_block_that_is_no_portal_puts_one_in_the_middle_along_x() {
    let mut world = World::empty();
    world.put(at(0, 64, 0), blocks::STONE);
    let read = |pos| world.block(pos);
    assert_eq!(
        exit_relative(at(0, 64, 0), [3.0, 70.0, 3.0], PLAYER, &read),
        (PortalAxis::X, [0.5, 0.0, 0.0])
    );
    let unreadable = |_| None;
    assert_eq!(
        exit_relative(at(0, 64, 0), [3.0, 70.0, 3.0], PLAYER, &unreadable),
        (PortalAxis::X, [0.5, 0.0, 0.0])
    );
}

// exit_pose

#[test]
fn through_portals_of_one_axis_the_pose_keeps_the_yaw_and_the_velocity() {
    let rectangle = two_by_three(at(0, 64, 0));
    let (position, velocity, yaw) = exit_pose(
        &rectangle,
        PortalAxis::X,
        [0.5, 0.0, 0.0],
        PortalAxis::X,
        PLAYER,
        [0.1, -0.08, 0.2],
        -30.0,
    );
    // Along: 0.3 + 1.4 × 0.5 = 1; up: 0; across: 0.5.
    assert_close(position, [1.0, 64.0, 0.5]);
    assert_eq!(velocity, [0.1, -0.08, 0.2]);
    assert_eq!(yaw, -30.0);
}

#[test]
fn through_portals_of_crossed_axes_the_yaw_and_the_velocity_turn_by_ninety() {
    let rectangle = FoundRectangle {
        min_corner: at(10, 70, 20),
        axis1_size: 2,
        axis2_size: 3,
    };
    let (position, velocity, yaw) = exit_pose(
        &rectangle,
        PortalAxis::Z,
        [0.25, 0.5, 0.2],
        PortalAxis::X,
        PLAYER,
        [0.1, 0.0, -0.2],
        45.0,
    );
    // Along z: 0.3 + 1.4 × 0.25 = 0.65; up: 1.2 × 0.5 = 0.6; across x: 0.5 + 0.2.
    assert_close(position, [10.7, 70.6, 20.65]);
    // Turned as the yaw is: (x, z) becomes (-z, x), as `Vec3.yRot(-π/2)` makes it.
    assert_eq!(velocity, [0.2, 0.0, 0.1]);
    assert_eq!(yaw, 135.0);
    // And from Z to X the same way round.
    let (_, velocity, yaw) = exit_pose(
        &rectangle,
        PortalAxis::X,
        [0.0, 0.0, 0.0],
        PortalAxis::Z,
        PLAYER,
        [0.0, 0.3, 0.0],
        170.0,
    );
    assert_eq!(yaw, 260.0);
    // No turn gives a negative zero, as the game's sum does not.
    assert_eq!(
        velocity.map(f64::to_bits),
        [0.0, 0.3, 0.0].map(f64::to_bits)
    );
}

#[test]
fn a_pose_carried_through_two_portals_of_one_size_lands_where_it_left() {
    let source = two_by_three(at(0, 64, 0));
    let relative = relative_position(&source, PortalAxis::X, [0.9, 65.0, 0.6], PLAYER);
    let destination = two_by_three(at(100, 30, -40));
    let (position, _, _) = exit_pose(
        &destination,
        PortalAxis::X,
        relative,
        PortalAxis::X,
        PLAYER,
        [0.0; 3],
        0.0,
    );
    assert_close(position, [100.9, 31.0, -39.4]);
}

// free_position

#[test]
fn a_player_in_the_open_stays_where_it_is() {
    let world = World::empty();
    let read = |pos| world.block(pos);
    let position = [0.5, 64.0, 0.5];
    assert_close(
        free_position(position, PLAYER, &read, DEFAULT_BORDER),
        position,
    );
}

#[test]
fn a_player_half_in_a_wall_is_moved_out_of_it() {
    // Ground of stone up to y = 63, and a wall at x = 1 from y = 64 to 66. The player
    // at x = 0.9 reaches 0.3 into the wall's block; it is moved back to x = 0.7.
    let mut world = World::new(|_, _| Some(63));
    for y in 64..67 {
        for z in -3..4 {
            world.put(at(1, y, z), blocks::STONE);
        }
    }
    let read = |pos| world.block(pos);
    let moved = free_position([0.9, 64.0, 0.5], PLAYER, &read, DEFAULT_BORDER);
    assert_close(moved, [0.7, 64.0, 0.5]);
}

#[test]
fn a_player_sunk_into_a_slab_is_lifted_onto_it() {
    // A bottom slab is half a block high: the player is lifted from 64.2 to 64.5,
    // which is within the one block up it may move.
    let mut world = World::empty();
    world.put(at(0, 64, 0), blocks::OAK_SLAB);
    assert_eq!(blocks::OAK_SLAB.property("type"), Some("bottom"));
    let read = |pos| world.block(pos);
    let moved = free_position([0.5, 64.2, 0.5], PLAYER, &read, DEFAULT_BORDER);
    assert_close(moved, [0.5, 64.5, 0.5]);
}

#[test]
fn a_player_buried_in_stone_is_left_where_it_is() {
    let world = World::new(|_, _| Some(200));
    let read = |pos| world.block(pos);
    let position = [0.5, 64.0, 0.5];
    assert_eq!(
        free_position(position, PLAYER, &read, DEFAULT_BORDER),
        position
    );
}

#[test]
fn an_entity_larger_than_four_is_not_moved() {
    let world = World::new(|_, _| Some(64));
    let read = |pos| world.block(pos);
    let position = [0.5, 64.2, 0.5];
    assert_eq!(
        free_position(position, (4.5, 1.0), &read, DEFAULT_BORDER),
        position
    );
    assert_eq!(
        free_position(position, (1.0, 4.1), &read, DEFAULT_BORDER),
        position
    );
}

#[test]
fn a_block_that_cannot_be_read_collides_with_nothing() {
    let read = |_| None;
    let position = [0.9, 64.0, 0.5];
    assert_close(
        free_position(position, PLAYER, &read, DEFAULT_BORDER),
        position,
    );
}

// The spiral

#[test]
fn the_spiral_covers_the_square_from_its_centre_outwards() {
    let columns = spiral_around(at(0, 7, 0), 16, (1, 0), (0, 1));
    assert_eq!(columns.len(), 33 * 33);
    let first: Vec<(i32, i32)> = columns.iter().take(10).map(|pos| (pos.x, pos.z)).collect();
    assert_eq!(
        first,
        [
            (0, 0),
            (1, 0),
            (1, 1),
            (0, 1),
            (-1, 1),
            (-1, 0),
            (-1, -1),
            (0, -1),
            (1, -1),
            (2, -1),
        ]
    );
    let distinct: BTreeSet<(i32, i32)> = columns.iter().map(|pos| (pos.x, pos.z)).collect();
    assert_eq!(distinct.len(), 33 * 33);
    assert!(
        columns
            .iter()
            .all(|pos| pos.x.abs() <= 16 && pos.z.abs() <= 16 && pos.y == 7)
    );
}

// make_portal

#[test]
fn a_portal_made_on_flat_ground_stands_on_it_at_the_target() {
    let world = World::new(|_, _| Some(63));
    for axis in [PortalAxis::X, PortalAxis::Z] {
        let (changes, rectangle, made_axis) = world
            .make(at(0, 64, 0), axis, 384)
            .expect("there is room on flat ground");
        assert_eq!(made_axis, axis);
        assert_eq!(rectangle, two_by_three(at(0, 64, 0)));
        assert_eq!(changes, frame_and_portal(at(0, 64, 0), axis));
    }
}

#[test]
fn a_portal_made_above_flat_ground_comes_down_onto_it() {
    let world = World::new(|_, _| Some(63));
    let (_, rectangle, _) = world.make(at(5, 90, -5), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(5, 64, -5));
}

#[test]
fn on_a_strip_one_block_wide_a_one_deep_spot_is_taken() {
    // Ground only along z = 0: no spot has ground on both sides of the frame.
    let world = World::new(|_, z| (z == 0).then_some(63));
    let (changes, rectangle, _) = world.make(at(3, 64, 0), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(3, 64, 0));
    assert_eq!(changes, frame_and_portal(at(3, 64, 0), PortalAxis::X));
}

#[test]
fn a_three_deep_spot_is_taken_over_a_nearer_one_deep_one() {
    // The strip along z = 0, and a patch x = 8..12, z = −2..2: the spots with ground
    // on both sides and under the whole frame are x = 9 and 10, z = −1..1; the nearest
    // to (0, 64, 0) is (9, 64, 0), 81 away, though the strip has one 0 away.
    let world =
        World::new(|x, z| (z == 0 || ((8..=12).contains(&x) && z.abs() <= 2)).then_some(63));
    let (_, rectangle, _) = world.make(at(0, 64, 0), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(9, 64, 0));
}

#[test]
fn along_z_the_three_deep_spot_needs_ground_on_both_sides_on_x() {
    // The same patch turned: x = −2..2 by z = 8..12, and the strip along x = 0.
    let world =
        World::new(|x, z| (x == 0 || ((8..=12).contains(&z) && x.abs() <= 2)).then_some(63));
    let (_, rectangle, _) = world.make(at(0, 64, 0), PortalAxis::Z, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(0, 64, 9));
    // Along X the strip has no room for a frame at all, and the patch's three-deep
    // spots along X are x = −1, 0, z = 9..11 (x − 1 and x + 2 within −2..2).
    let (_, rectangle, _) = world.make(at(0, 64, 0), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(0, 64, 9));
}

#[test]
fn at_a_cliffs_edge_the_frame_is_moved_onto_the_cliff() {
    // A plateau up to y = 80 for x ≥ 3, ground up to 63 below it. At (3, 81, 0) the
    // frame would hang over the edge at x = 2; the nearest spot is one block in.
    let world = World::new(|x, _| Some(if x >= 3 { 80 } else { 63 }));
    let (changes, rectangle, _) = world.make(at(3, 81, 0), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(4, 81, 0));
    assert_eq!(changes, frame_and_portal(at(4, 81, 0), PortalAxis::X));
    // Along Z the frame lies along the edge: the target's spot has ground on one
    // side only, so the three-deep spot one block in is taken over it.
    let (_, rectangle, _) = world.make(at(3, 81, 0), PortalAxis::Z, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(4, 81, 0));
}

#[test]
fn below_a_cliff_a_spot_too_near_its_face_is_passed_over() {
    // Below the plateau (x ≥ 3, up to 80), at (1, 64, 0): along X the frame from the
    // target reaches x = 3, the plateau's stone. The spot at x = 0, whose frame ends
    // at x = 2, is the nearest with room, 1 away.
    let world = World::new(|x, _| Some(if x >= 3 { 80 } else { 63 }));
    let (_, rectangle, _) = world.make(at(1, 64, 0), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(0, 64, 0));
}

#[test]
fn in_solid_stone_a_platform_is_made_at_seventy_or_the_target_if_higher() {
    let world = World::new(|_, _| Some(400));
    let (changes, rectangle, _) = world.make(at(0, 40, 0), PortalAxis::X, 384).expect("room");
    // Along X the spot is one step west of the target, at y = clamp(40, 70, 310).
    let spot = at(-1, 70, 0);
    assert_eq!(rectangle, two_by_three(spot));
    let mut expected = Vec::new();
    for side in -1..=1 {
        for along in 0..=1 {
            for up in -1..=2 {
                let state = if up < 0 {
                    blocks::OBSIDIAN
                } else {
                    blocks::AIR
                };
                expected.push((at(spot.x + along, spot.y + up, spot.z + side), state));
            }
        }
    }
    expected.extend(frame_and_portal(spot, PortalAxis::X));
    assert_eq!(changes, expected);

    let (_, rectangle, _) = world.make(at(7, 200, 7), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(6, 200, 7));
    let (_, rectangle, _) = world.make(at(7, 315, 7), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(6, 310, 7));
}

#[test]
fn along_z_the_platform_is_set_from_its_east_side_and_one_step_north_of_the_target() {
    let world = World::new(|_, _| Some(400));
    let (changes, rectangle, _) = world.make(at(0, 40, 0), PortalAxis::Z, 384).expect("room");
    let spot = at(0, 70, -1);
    assert_eq!(rectangle, two_by_three(spot));
    // The clockwise turn of south is west, so the first side, −1, is east.
    let first: Vec<BlockPos> = changes.iter().take(5).map(|(pos, _)| *pos).collect();
    assert_eq!(
        first,
        [
            at(1, 69, -1),
            at(1, 70, -1),
            at(1, 71, -1),
            at(1, 72, -1),
            at(1, 69, 0)
        ]
    );
    assert_eq!(changes.len(), 24 + 14 + 6);
}

#[test]
fn in_the_nether_the_platform_stays_nine_below_the_roof() {
    // The Nether: lowest block 0, logical height 128, so the top is 127 and the
    // platform no higher than 118.
    let mut world = World::new(|_, _| Some(400));
    world.min_y = 0;
    let (_, rectangle, _) = world.make(at(0, 125, 0), PortalAxis::X, 128).expect("room");
    assert_eq!(rectangle.min_corner, at(-1, 118, 0));
    let (_, rectangle, _) = world.make(at(0, 10, 0), PortalAxis::X, 128).expect("room");
    assert_eq!(rectangle.min_corner, at(-1, 70, 0));
}

#[test]
fn no_portal_is_made_where_the_level_leaves_no_room_for_the_platform() {
    // Lowest block 0 and logical height 64: the top is 63, and 63 − 9 is below 70.
    let mut world = World::new(|_, _| Some(400));
    world.min_y = 0;
    assert_eq!(world.make(at(0, 30, 0), PortalAxis::X, 64), None);
    // With 80 the top is 79 and there is just room, at 70.
    let (_, rectangle, _) = world.make(at(0, 30, 0), PortalAxis::X, 80).expect("room");
    assert_eq!(rectangle.min_corner, at(-1, 70, 0));
}

#[test]
fn where_the_border_leaves_no_column_to_look_at_the_platform_is_held_within_it() {
    // Every column about (100, 100) is beyond a border of 8, so none is looked at, on
    // flat ground: the platform's spot (99, 70, 100) is held to x and z of 8 − 1e-5,
    // floored to 7.
    let world = World::new(|_, _| Some(63));
    let (changes, rectangle, _) = world
        .make_within(at(100, 64, 100), PortalAxis::X, 384, 8.0)
        .expect("room");
    assert_eq!(rectangle.min_corner, at(7, 70, 7));
    assert_eq!(changes[0], (at(7, 69, 6), blocks::OBSIDIAN));
}

#[test]
fn a_column_whose_step_along_the_axis_is_beyond_the_border_is_not_looked_at() {
    // A border of 3: x = 2 is within it and x = 3 not, so along X no column at x = 2
    // is looked at; the target's column (2, 0) is not, and (1, 0) is taken.
    let world = World::new(|_, _| Some(63));
    let (_, rectangle, _) = world
        .make_within(at(2, 64, 0), PortalAxis::X, 384, 3.0)
        .expect("room");
    assert_eq!(rectangle.min_corner, at(1, 64, 0));
}

#[test]
fn a_column_with_a_block_that_cannot_be_read_is_passed_over() {
    // The target's column reads a block deep in the stone that cannot be read, so the
    // whole column is unsuitable; of the spots 1 away the spiral's first, east, wins.
    let mut world = World::new(|_, _| Some(63));
    world.unreadable.insert(at(0, 40, 0));
    let (_, rectangle, _) = world.make(at(0, 64, 0), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(1, 64, 0));
}

#[test]
fn a_column_whose_height_cannot_be_read_is_passed_over() {
    let world = World::new(|_, _| Some(63));
    let (_, rectangle, _) = make_portal(
        at(0, 64, 0),
        PortalAxis::X,
        &|pos| world.block(pos),
        &|x, z| ((x, z) != (0, 0)).then(|| world.height(x, z)).flatten(),
        world.min_y,
        384,
        DEFAULT_BORDER,
    )
    .expect("room");
    assert_eq!(rectangle.min_corner, at(1, 64, 0));
}

#[test]
fn water_is_not_replaced_by_a_portal() {
    // A sea of water up to 70 over ground at 63: no spot in the sea, so the platform
    // is made at 70, one block west of the target.
    let mut world = World::new(|_, _| Some(63));
    for x in -20..=20 {
        for z in -20..=20 {
            for y in 64..=70 {
                world.put(at(x, y, z), blocks::WATER);
            }
        }
    }
    let heights = |_: i32, _: i32| Some(71);
    let (_, rectangle, _) = make_portal(
        at(0, 64, 0),
        PortalAxis::X,
        &|pos| world.block(pos),
        &heights,
        world.min_y,
        384,
        DEFAULT_BORDER,
    )
    .expect("room");
    // The sea's surface at y = 71 is open air on water, which is not solid: no spot.
    assert_eq!(rectangle.min_corner, at(-1, 70, 0));
}

#[test]
fn grass_on_the_ground_is_replaced_by_the_frame() {
    // Short grass can be replaced, so the spot is on the grass's block, standing on
    // the stone.
    let mut world = World::new(|_, _| Some(63));
    for x in -3..=4 {
        for z in -2..=2 {
            world.put(at(x, 64, z), blocks::SHORT_GRASS);
        }
    }
    let (_, rectangle, _) = world.make(at(0, 64, 0), PortalAxis::X, 384).expect("room");
    assert_eq!(rectangle.min_corner, at(0, 64, 0));
}
