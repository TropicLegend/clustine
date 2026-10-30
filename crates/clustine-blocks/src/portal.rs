//! The frame of a Nether portal (the game's `PortalShape`, PortalShape.txt): where the
//! rectangle inside a frame of obsidian is, and whether it is empty or full of portal.
//! Lighting a frame asks for an empty one (`findEmptyPortalShape`) and fills it
//! (`createPortalBlocks`); a portal block whose neighbour changes asks for a full one
//! (`findAnyShape(..).isComplete()`, `NetherPortalBlock.updateShape`).
//!
//! The search reads blocks through a function and nothing else, so that a caller who
//! cannot see some of them can tell (the simulation keeps a portal it cannot judge).

use clustine_data::{BlockState, Face, blocks};
use clustine_world::{BlockPos, PortalAxis};

use crate::data::{in_tag, is_block, relative, set};

/// `MIN_WIDTH` and `MAX_WIDTH`: the inside's width, without the frame.
pub const MIN_WIDTH: i32 = 2;
pub const MAX_WIDTH: i32 = 21;
/// `MIN_HEIGHT` and `MAX_HEIGHT`: the inside's height, without the frame.
pub const MIN_HEIGHT: i32 = 3;
pub const MAX_HEIGHT: i32 = 21;

/// `PortalShape.FRAME`: the blocks a frame is made of (`#nether_portal_frame`, which is
/// obsidian alone).
pub fn is_frame(state: BlockState) -> bool {
    in_tag(state, "minecraft:nether_portal_frame")
}

/// `PortalShape.isEmpty`: air, fire of any kind (`#fire`) or portal, all of which a
/// frame may be lit over.
pub fn is_empty(state: BlockState) -> bool {
    state.props().is_air()
        || in_tag(state, "minecraft:fire")
        || is_block(state, blocks::NETHER_PORTAL)
}

/// A rectangle found inside a frame: `PortalShape`'s fields. A search that finds none
/// has a width or height of 0 and is not valid, as the game's has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalShape {
    pub axis: PortalAxis,
    /// `numPortalBlocks`: how many portal blocks the rectangle already holds.
    pub portal_blocks: i32,
    /// `bottomLeft`: the lowest block of the inside on the left, which is east of the
    /// rectangle for an X axis and north of it for a Z axis.
    pub bottom_left: BlockPos,
    pub width: i32,
    pub height: i32,
}

impl PortalShape {
    /// `isValid`: 2 to 21 wide and 3 to 21 high.
    pub fn is_valid(&self) -> bool {
        (MIN_WIDTH..=MAX_WIDTH).contains(&self.width)
            && (MIN_HEIGHT..=MAX_HEIGHT).contains(&self.height)
    }

    /// `isComplete`: valid, and every block inside it portal.
    pub fn is_complete(&self) -> bool {
        self.is_valid() && self.portal_blocks == self.width * self.height
    }

    /// The portal block of the shape's axis (`NETHER_PORTAL` with `AXIS`).
    pub fn portal_state(&self) -> BlockState {
        set(blocks::NETHER_PORTAL, "axis", axis_name(self.axis))
    }

    /// The blocks `createPortalBlocks` sets, in its order: `BlockPos.betweenClosed` from
    /// the bottom left to the top right, x fastest, then y, then z.
    pub fn cells(&self) -> Vec<BlockPos> {
        let right = right_direction(self.axis);
        let top = BlockPos::new(
            self.bottom_left.x,
            self.bottom_left.y + self.height - 1,
            self.bottom_left.z,
        );
        let far = moved(top, right, self.width - 1);
        let (low, high) = (
            BlockPos::new(
                self.bottom_left.x.min(far.x),
                self.bottom_left.y.min(far.y),
                self.bottom_left.z.min(far.z),
            ),
            BlockPos::new(
                self.bottom_left.x.max(far.x),
                self.bottom_left.y.max(far.y),
                self.bottom_left.z.max(far.z),
            ),
        );
        let mut cells = Vec::new();
        for z in low.z..=high.z {
            for y in low.y..=high.y {
                for x in low.x..=high.x {
                    cells.push(BlockPos::new(x, y, z));
                }
            }
        }
        cells
    }
}

/// The name of `axis` as the block's property has it.
pub fn axis_name(axis: PortalAxis) -> &'static str {
    match axis {
        PortalAxis::X => "x",
        PortalAxis::Z => "z",
    }
}

/// The axis of a portal block (`AXIS`), X where it says nothing.
pub fn axis_of(state: BlockState) -> PortalAxis {
    match state.property("axis") {
        Some("z") => PortalAxis::Z,
        _ => PortalAxis::X,
    }
}

/// The other horizontal axis.
pub fn other_axis(axis: PortalAxis) -> PortalAxis {
    match axis {
        PortalAxis::X => PortalAxis::Z,
        PortalAxis::Z => PortalAxis::X,
    }
}

/// `rightDir` of `findAnyShape`: west along an X axis, south along a Z axis.
fn right_direction(axis: PortalAxis) -> Face {
    match axis {
        PortalAxis::X => Face::West,
        PortalAxis::Z => Face::South,
    }
}

/// `pos.relative(direction, distance)`.
fn moved(pos: BlockPos, direction: Face, distance: i32) -> BlockPos {
    let one = relative(BlockPos::new(0, 0, 0), direction);
    BlockPos::new(
        pos.x + one.x * distance,
        pos.y + one.y * distance,
        pos.z + one.z * distance,
    )
}

/// `findEmptyPortalShape(level, pos, axis)`: the valid shape about `pos` with no portal
/// in it yet, along `axis` first and then along the other (`findPortalShape`), if there
/// is one. `min_y` is the level's lowest block (`getMinY`).
pub fn find_empty_portal_shape(
    read: &mut dyn FnMut(BlockPos) -> BlockState,
    min_y: i32,
    pos: BlockPos,
    axis: PortalAxis,
) -> Option<PortalShape> {
    let empty = |shape: &PortalShape| shape.is_valid() && shape.portal_blocks == 0;
    let first = find_any_shape(read, min_y, pos, axis);
    if empty(&first) {
        return Some(first);
    }
    let second = find_any_shape(read, min_y, pos, other_axis(axis));
    empty(&second).then_some(second)
}

/// `findAnyShape(level, pos, axis)`: the rectangle inside the frame about `pos`, valid
/// or not.
pub fn find_any_shape(
    read: &mut dyn FnMut(BlockPos) -> BlockState,
    min_y: i32,
    pos: BlockPos,
    axis: PortalAxis,
) -> PortalShape {
    let right = right_direction(axis);
    let none = |bottom_left| PortalShape {
        axis,
        portal_blocks: 0,
        bottom_left,
        width: 0,
        height: 0,
    };
    let Some(bottom_left) = bottom_left(read, min_y, right, pos) else {
        return none(pos);
    };
    let width = width(read, bottom_left, right);
    if width == 0 {
        return none(bottom_left);
    }
    let mut portal_blocks = 0;
    let height = height(read, bottom_left, right, width, &mut portal_blocks);
    PortalShape {
        axis,
        portal_blocks,
        bottom_left,
        width,
        height,
    }
}

/// `calculateBottomLeft`: down through the empty blocks below `pos`, no lower than 21
/// below it or the level's floor, then left to the frame; none where no frame is to the
/// left.
fn bottom_left(
    read: &mut dyn FnMut(BlockPos) -> BlockState,
    min_y: i32,
    right: Face,
    pos: BlockPos,
) -> Option<BlockPos> {
    let lowest = min_y.max(pos.y - 21);
    let mut pos = pos;
    while pos.y > lowest && is_empty(read(relative(pos, Face::Down))) {
        pos = relative(pos, Face::Down);
    }
    let left = right.opposite();
    let distance = distance_until_edge_above_frame(read, pos, left) - 1;
    (distance >= 0).then(|| moved(pos, left, distance))
}

/// `calculateWidth`: the distance to the frame on the right, if it is 2 to 21; else 0.
fn width(read: &mut dyn FnMut(BlockPos) -> BlockState, bottom_left: BlockPos, right: Face) -> i32 {
    let width = distance_until_edge_above_frame(read, bottom_left, right);
    if (MIN_WIDTH..=MAX_WIDTH).contains(&width) {
        width
    } else {
        0
    }
}

/// `getDistanceUntilEdgeAboveFrame`: along `direction` from `pos`, up to 21 steps on,
/// over empty blocks that stand on frame, the distance to the first block that is not
/// empty, if it is frame; else 0.
fn distance_until_edge_above_frame(
    read: &mut dyn FnMut(BlockPos) -> BlockState,
    pos: BlockPos,
    direction: Face,
) -> i32 {
    for i in 0..=MAX_WIDTH {
        let at = moved(pos, direction, i);
        let state = read(at);
        if !is_empty(state) {
            return if is_frame(state) { i } else { 0 };
        }
        if !is_frame(read(relative(at, Face::Down))) {
            return 0;
        }
    }
    0
}

/// `calculateHeight`: the height of the columns of empty blocks between two posts of
/// frame, accepted if it is 3 to 21 and the row above it is frame all along; else 0.
/// Counts the portal blocks inside into `portal_blocks`.
fn height(
    read: &mut dyn FnMut(BlockPos) -> BlockState,
    bottom_left: BlockPos,
    right: Face,
    width: i32,
    portal_blocks: &mut i32,
) -> i32 {
    let height = distance_until_top(read, bottom_left, right, width, portal_blocks);
    if (MIN_HEIGHT..=MAX_HEIGHT).contains(&height)
        && has_top_frame(read, bottom_left, right, width, height)
    {
        height
    } else {
        0
    }
}

/// `hasTopFrame`: the row `height` above the bottom left is frame across the width. The
/// corners are not looked at.
fn has_top_frame(
    read: &mut dyn FnMut(BlockPos) -> BlockState,
    bottom_left: BlockPos,
    right: Face,
    width: i32,
    height: i32,
) -> bool {
    let row = BlockPos::new(bottom_left.x, bottom_left.y + height, bottom_left.z);
    (0..width).all(|i| is_frame(read(moved(row, right, i))))
}

/// `getDistanceUntilTop`: up from the bottom row, the first row without frame on the
/// left (one to the left) and on the right (`width` to the right), or with a block inside
/// that is not empty; 21 where every row up to 21 is whole. The portal blocks of the
/// rows before it are counted, and so are those of that row that come before the block
/// that ends it, as the game counts them.
fn distance_until_top(
    read: &mut dyn FnMut(BlockPos) -> BlockState,
    bottom_left: BlockPos,
    right: Face,
    width: i32,
    portal_blocks: &mut i32,
) -> i32 {
    for h in 0..MAX_HEIGHT {
        let row = BlockPos::new(bottom_left.x, bottom_left.y + h, bottom_left.z);
        if !is_frame(read(moved(row, right, -1))) {
            return h;
        }
        if !is_frame(read(moved(row, right, width))) {
            return h;
        }
        for i in 0..width {
            let state = read(moved(row, right, i));
            if !is_empty(state) {
                return h;
            }
            if is_block(state, blocks::NETHER_PORTAL) {
                *portal_blocks += 1;
            }
        }
    }
    MAX_HEIGHT
}

/// `NetherPortalBlock.updateShape`: whether the portal block `state` at `pos` stays as
/// its neighbour towards `direction` becomes `neighbour`. It stays where the change is
/// in front of it or behind it (a horizontal direction across its axis), where the
/// neighbour is portal, or where the frame about it is still whole and full; else it
/// becomes air, and so does the rest of the portal as the change reaches it.
pub fn portal_stays(
    read: &mut dyn FnMut(BlockPos) -> BlockState,
    min_y: i32,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> bool {
    let axis = axis_of(state);
    let across = match direction {
        Face::East | Face::West => axis != PortalAxis::X,
        Face::North | Face::South => axis != PortalAxis::Z,
        Face::Up | Face::Down => false,
    };
    across
        || is_block(neighbour, blocks::NETHER_PORTAL)
        || find_any_shape(read, min_y, pos, axis).is_complete()
}

#[cfg(test)]
#[path = "portal_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "portal_specification.rs"]
mod specification;
