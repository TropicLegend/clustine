//! Tests of the Nether portal's frame from the game's rules (`docs/groundwork/p-brief.md`,
//! section 4.1; PortalShape.txt): its sizes, the corners never looked at, fire and portal
//! counted as empty, and the search's way down and to the left.

use std::collections::BTreeMap;

use clustine_data::{BlockState, Face, blocks};
use clustine_world::{BlockPos, PortalAxis};

use super::*;

const MIN_Y: i32 = -64;

/// Blocks by position, air elsewhere.
#[derive(Default)]
struct World(BTreeMap<BlockPos, BlockState>);

impl World {
    fn put(&mut self, pos: BlockPos, state: BlockState) {
        self.0.insert(pos, state);
    }

    fn read(&self) -> impl FnMut(BlockPos) -> BlockState + '_ {
        |pos| self.0.get(&pos).copied().unwrap_or(blocks::AIR)
    }

    fn empty_shape(&self, pos: BlockPos, axis: PortalAxis) -> Option<PortalShape> {
        find_empty_portal_shape(&mut self.read(), MIN_Y, pos, axis)
    }

    fn any_shape(&self, pos: BlockPos, axis: PortalAxis) -> PortalShape {
        find_any_shape(&mut self.read(), MIN_Y, pos, axis)
    }
}

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// A frame of obsidian along `axis` whose inside is `width` by `height` with its lowest
/// block at `low` and running towards +x (axis X) or +z (axis Z), without its corners.
fn frame(world: &mut World, axis: PortalAxis, low: BlockPos, width: i32, height: i32) {
    let along = |pos: BlockPos, i: i32| match axis {
        PortalAxis::X => at(pos.x + i, pos.y, pos.z),
        PortalAxis::Z => at(pos.x, pos.y, pos.z + i),
    };
    for i in 0..width {
        world.put(along(at(low.x, low.y - 1, low.z), i), blocks::OBSIDIAN);
        world.put(along(at(low.x, low.y + height, low.z), i), blocks::OBSIDIAN);
    }
    for h in 0..height {
        world.put(along(at(low.x, low.y + h, low.z), -1), blocks::OBSIDIAN);
        world.put(along(at(low.x, low.y + h, low.z), width), blocks::OBSIDIAN);
    }
}

#[test]
fn the_smallest_frame_is_found_from_any_block_inside_it_with_its_bottom_left_east() {
    let mut world = World::default();
    frame(&mut world, PortalAxis::X, at(0, 0, 0), 2, 3);
    for pos in [at(0, 0, 0), at(1, 0, 0), at(0, 2, 0), at(1, 1, 0)] {
        let shape = world.empty_shape(pos, PortalAxis::X).expect("a shape");
        assert_eq!(shape.axis, PortalAxis::X);
        assert_eq!((shape.width, shape.height), (2, 3));
        // The right is west along X, so the bottom left is the eastern block.
        assert_eq!(shape.bottom_left, at(1, 0, 0));
        assert_eq!(shape.portal_blocks, 0);
        assert_eq!(
            shape.cells(),
            vec![
                at(0, 0, 0),
                at(1, 0, 0),
                at(0, 1, 0),
                at(1, 1, 0),
                at(0, 2, 0),
                at(1, 2, 0),
            ]
        );
    }
}

#[test]
fn a_frame_along_z_is_found_after_the_x_axis_fails_and_its_bottom_left_is_north() {
    let mut world = World::default();
    frame(&mut world, PortalAxis::Z, at(4, 10, 4), 3, 4);
    let shape = world
        .empty_shape(at(4, 12, 5), PortalAxis::X)
        .expect("a shape");
    assert_eq!(shape.axis, PortalAxis::Z);
    assert_eq!((shape.width, shape.height), (3, 4));
    assert_eq!(shape.bottom_left, at(4, 10, 4));
    assert_eq!(
        shape.portal_state(),
        set(blocks::NETHER_PORTAL, "axis", "z")
    );
    assert!(!world.any_shape(at(4, 12, 5), PortalAxis::X).is_valid());
}

#[test]
fn widths_from_2_to_21_and_heights_from_3_to_21_are_valid_and_no_others() {
    for (width, height, valid) in [
        (1, 3, false),
        (2, 3, true),
        (2, 2, false),
        (21, 21, true),
        (22, 3, false),
        (2, 22, false),
        (21, 3, true),
        (2, 21, true),
    ] {
        let mut world = World::default();
        frame(&mut world, PortalAxis::X, at(0, 0, 0), width, height);
        let found = world.empty_shape(at(0, 0, 0), PortalAxis::X);
        assert_eq!(found.is_some(), valid, "{width} by {height}");
        if let Some(shape) = found {
            assert_eq!((shape.width, shape.height), (width, height));
        }
    }
}

#[test]
fn the_corners_of_a_frame_are_never_looked_at() {
    let mut world = World::default();
    frame(&mut world, PortalAxis::X, at(0, 0, 0), 2, 3);
    for corner in [at(-1, -1, 0), at(2, -1, 0), at(-1, 3, 0), at(2, 3, 0)] {
        world.put(corner, blocks::DIRT);
    }
    assert!(world.empty_shape(at(0, 1, 0), PortalAxis::X).is_some());
    for corner in [at(-1, -1, 0), at(2, -1, 0), at(-1, 3, 0), at(2, 3, 0)] {
        world.put(corner, blocks::AIR);
    }
    assert!(world.empty_shape(at(0, 1, 0), PortalAxis::X).is_some());
}

#[test]
fn fire_inside_counts_as_empty_and_portal_inside_makes_the_frame_not_empty_but_complete() {
    let mut world = World::default();
    frame(&mut world, PortalAxis::X, at(0, 0, 0), 2, 3);
    world.put(at(0, 0, 0), blocks::FIRE);
    world.put(at(1, 2, 0), blocks::SOUL_FIRE);
    assert!(world.empty_shape(at(0, 0, 0), PortalAxis::X).is_some());

    let portal = set(blocks::NETHER_PORTAL, "axis", "x");
    world.put(at(1, 2, 0), portal);
    assert!(world.empty_shape(at(0, 0, 0), PortalAxis::X).is_none());
    let partly = world.any_shape(at(0, 0, 0), PortalAxis::X);
    assert!(partly.is_valid() && !partly.is_complete());
    assert_eq!(partly.portal_blocks, 1);

    for cell in partly.cells() {
        world.put(cell, portal);
    }
    let full = world.any_shape(at(1, 1, 0), PortalAxis::X);
    assert!(full.is_complete());
    assert_eq!(full.portal_blocks, 6);
}

#[test]
fn a_block_inside_that_is_not_empty_ends_the_frame_below_it() {
    let mut world = World::default();
    frame(&mut world, PortalAxis::X, at(0, 0, 0), 2, 3);
    world.put(at(1, 2, 0), blocks::DIRT);
    // The rows below the dirt are two high, which is too few.
    assert!(world.empty_shape(at(0, 0, 0), PortalAxis::X).is_none());
}

#[test]
fn the_search_goes_down_to_the_frame_and_left_only_over_blocks_standing_on_frame() {
    let mut world = World::default();
    frame(&mut world, PortalAxis::X, at(0, 0, 0), 3, 5);
    let from_top = world
        .empty_shape(at(0, 4, 0), PortalAxis::X)
        .expect("found from the top row");
    assert_eq!(from_top.bottom_left, at(2, 0, 0));
    // A gap in the frame's floor below the inside: the walk left from the west end
    // meets a block that stands on nothing and there is no shape.
    world.put(at(1, -1, 0), blocks::AIR);
    assert!(world.empty_shape(at(0, 4, 0), PortalAxis::X).is_none());
}

#[test]
fn the_search_goes_no_lower_than_21_below_where_it_starts() {
    let mut world = World::default();
    frame(&mut world, PortalAxis::X, at(0, 0, 0), 2, 21);
    // From the top row the floor is 20 below: found.
    assert!(world.empty_shape(at(0, 20, 0), PortalAxis::X).is_some());
    // A frame too tall to be valid is not found from anywhere in it.
    let mut tall = World::default();
    frame(&mut tall, PortalAxis::X, at(0, 0, 0), 2, 23);
    assert!(tall.empty_shape(at(0, 22, 0), PortalAxis::X).is_none());
    assert!(tall.empty_shape(at(0, 0, 0), PortalAxis::X).is_none());
}

#[test]
fn the_search_stops_at_the_floor_of_the_level() {
    let mut world = World::default();
    // Obsidian below the floor would be found if the search went below it.
    frame(&mut world, PortalAxis::X, at(0, MIN_Y - 1, 0), 2, 3);
    let shape = world.any_shape(at(0, MIN_Y + 1, 0), PortalAxis::X);
    assert!(!shape.is_valid());
}

#[test]
fn a_portal_block_stays_for_a_change_in_front_or_behind_or_of_portal_and_goes_with_its_frame() {
    let mut world = World::default();
    frame(&mut world, PortalAxis::X, at(0, 0, 0), 2, 3);
    let portal = set(blocks::NETHER_PORTAL, "axis", "x");
    for cell in world.any_shape(at(0, 0, 0), PortalAxis::X).cells() {
        world.put(cell, portal);
    }
    let pos = at(0, 1, 0);
    let stays = |world: &World, direction: Face, neighbour: BlockState| {
        portal_stays(&mut world.read(), MIN_Y, portal, pos, direction, neighbour)
    };
    // Whole and full.
    assert!(stays(&world, Face::Up, portal));
    assert!(stays(&world, Face::West, blocks::OBSIDIAN));
    // A frame block mined: along the axis or up and down, the shape is judged again.
    world.put(at(-1, 1, 0), blocks::AIR);
    assert!(!stays(&world, Face::West, blocks::AIR));
    assert!(!stays(&world, Face::Down, blocks::AIR));
    // Across the axis (north and south of an X portal) nothing is judged.
    assert!(stays(&world, Face::North, blocks::AIR));
    assert!(stays(&world, Face::South, blocks::DIRT));
    // A neighbour of portal keeps it, whatever the frame.
    assert!(stays(&world, Face::Up, portal));
    // A portal block gone from inside: the shape is no longer full.
    world.put(at(-1, 1, 0), blocks::OBSIDIAN);
    world.put(at(1, 1, 0), blocks::AIR);
    assert!(!stays(&world, Face::Up, blocks::AIR));
}
