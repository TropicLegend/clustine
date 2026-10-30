//! Where a Nether portal sends a traveller, as the game works it out: the portal it
//! chooses at the destination (`PortalForcer.findClosestPortalPosition`), the
//! rectangle of portal blocks about one (`BlockUtil.getLargestRectangleAround`), the
//! traveller's place within the portal it leaves and the one it reaches
//! (`PortalShape.getRelativePosition`, `NetherPortalBlock.createDimensionTransition`),
//! the nearest place where nothing collides with it (`PortalShape
//! .findCollisionFreePosition`, `CollisionGetter.findFreePosition`), and the portal the
//! game makes where it finds none (`PortalForcer.createPortal`). The line numbers are
//! those of the game's code as `javap -c` gives it (PortalForcer.txt and the others).
//!
//! Each is a function of the blocks it reads and nothing else: it knows no regions, no
//! ticks and no network, so that the region that places a traveller can ask it of the
//! blocks it has. A reader answers `None` for a block the caller cannot read; each
//! function says what it makes of that.

use clustine_data::{BlockState, OffsetType, blocks};
use clustine_world::{BlockPos, PortalAxis};

use crate::data::{is_block, set};
use crate::portal::axis_name;

/// Reads a block, or answers `None` where the caller cannot.
pub type BlockReader<'a> = &'a dyn Fn(BlockPos) -> Option<BlockState>;

/// Reads the heightmap `MOTION_BLOCKING` at a column (x, z): the y of the lowest block
/// above everything in it that blocks motion, as the game's `getHeight` answers; `None`
/// where the caller cannot.
pub type HeightReader<'a> = &'a dyn Fn(i32, i32) -> Option<i32>;

/// The world border's half-width about (0, 0) where nothing has changed it: the
/// default size of 59,999,968 halved (`WorldBorder`).
pub const DEFAULT_BORDER: f64 = 29_999_984.0;

/// `PortalForcer.NETHER_PORTAL_RADIUS` and `OVERWORLD_PORTAL_RADIUS` (PortalForcer.txt
/// 5–7): how far from the target a portal is looked for, in the Nether and in the
/// overworld.
pub const NETHER_PORTAL_RADIUS: i32 = 16;
pub const OVERWORLD_PORTAL_RADIUS: i32 = 128;

/// `BlockUtil.FoundRectangle`: a rectangle of blocks along a horizontal axis and up,
/// from its lowest corner on the axis's negative side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoundRectangle {
    pub min_corner: BlockPos,
    /// Its size along the horizontal axis.
    pub axis1_size: i32,
    /// Its size up.
    pub axis2_size: i32,
}

/// `PortalForcer.findClosestPortalPosition` (PortalForcer.txt 40–81), of the blocks
/// `getInSquare` finds: the portal blocks within `radius` of the target on x and on z
/// (`PoiManager.getInSquare`'s filter, PoiManager.txt 797–821) and within the border,
/// the nearest by `distSqr` to the target, and of those as near the lowest. The caller
/// gives only blocks that are still portals (the game's `hasProperty(HORIZONTAL_AXIS)`),
/// in the order it found them: of two at one distance and one height the first is kept,
/// as `Stream.min` keeps it.
///
/// The second key is a method reference whose target the dump does not show; it is
/// taken to be `getY`, which the brief infers (p-brief 2.2, item 3).
pub fn nearest_portal(
    candidates: &[BlockPos],
    target: BlockPos,
    radius: i32,
    border: f64,
) -> Option<BlockPos> {
    let mut best: Option<(f64, i32, BlockPos)> = None;
    for &pos in candidates {
        let in_square = (pos.x - target.x).abs() <= radius && (pos.z - target.z).abs() <= radius;
        if !in_square || !within_border(border, pos.x, pos.z) {
            continue;
        }
        let distance = dist_sqr(pos, target);
        let better = match best {
            None => true,
            Some((best_distance, best_y, _)) => {
                distance < best_distance || (distance == best_distance && pos.y < best_y)
            }
        };
        if better {
            best = Some((distance, pos.y, pos));
        }
    }
    best.map(|(_, _, pos)| pos)
}

/// `Vec3i.distSqr`: the square of the distance between two blocks, in doubles.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let dx = f64::from(a.x) - f64::from(b.x);
    let dy = f64::from(a.y) - f64::from(b.y);
    let dz = f64::from(a.z) - f64::from(b.z);
    dx * dx + dy * dy + dz * dz
}

/// `WorldBorder.isWithinBounds(x, z)` of a border of half-width `border` about (0, 0):
/// the low edges inside, the high ones out.
fn within_border(border: f64, x: i32, z: i32) -> bool {
    within_border_at(border, f64::from(x), f64::from(z))
}

fn within_border_at(border: f64, x: f64, z: f64) -> bool {
    x >= -border && x < border && z >= -border && z < border
}

/// `WorldBorder.clampToBounds(double, double, double)` of a border of half-width
/// `border` about (0, 0): x and z held within it, the high edge less 1e-5 (a float's),
/// and the point taken as the block it is in (`BlockPos.containing`). A Nether portal's
/// target is this of the traveller's scaled position (p-brief 2.2, item 2).
pub fn clamp_to_bounds(border: f64, x: f64, y: f64, z: f64) -> BlockPos {
    let high = border - f64::from(1.0e-5_f32);
    let clamp = |value: f64| clamp(value, -border, high).floor() as i32;
    BlockPos::new(clamp(x), y.floor() as i32, clamp(z))
}

/// `WorldBorder.clampToBounds(BlockPos)` (WorldBorder.txt 225–271): x and z held
/// within the border, the high edge less 1e-5 (a float's), then floored as
/// `BlockPos.containing` does.
fn clamp_to_border(border: f64, pos: BlockPos) -> BlockPos {
    let margin = f64::from(1.0e-5_f32);
    let clamp = |value: i32| {
        let value = f64::from(value);
        let high = border - margin;
        let clamped = if value < -border {
            -border
        } else {
            value.min(high)
        };
        clamped.floor() as i32
    };
    BlockPos::new(clamp(pos.x), pos.y, clamp(pos.z))
}

/// `BlockUtil.getLargestRectangleAround(block, axis, 21, Y, 21, same state)`
/// (BlockUtil.txt 11–539, and `getMaxRectangleLocation` 541–621), as both the portal a
/// traveller leaves (`getDimensionTransitionFromExit`, NetherPortalBlock.txt 332–380) and
/// the one it is sent to (`getExitPortal` 247–330) are measured: the largest rectangle
/// of blocks equal to `block`'s own state, along `axis` and up, that the rows and
/// columns through `block` reach, each at most 21 from it.
///
/// A block the reader cannot read is not equal to it, so an unreadable portal has the
/// rectangle of `block` alone (1 by 1), as one surrounded by other blocks has.
pub fn largest_rectangle(block: BlockPos, axis: PortalAxis, read: BlockReader) -> FoundRectangle {
    const LIMIT: i32 = 21;
    let state = read(block);
    let same = |pos: BlockPos| state.is_some() && read(pos) == state;
    let along = |pos: BlockPos, distance: i32| match axis {
        PortalAxis::X => BlockPos::new(pos.x + distance, pos.y, pos.z),
        PortalAxis::Z => BlockPos::new(pos.x, pos.y, pos.z + distance),
    };
    // `getLimit`: how many blocks on from `from`, one step at a time, up to `max`, pass.
    let limit = |from: BlockPos, step: &dyn Fn(BlockPos) -> BlockPos, max: i32| {
        let mut at = from;
        let mut count = 0;
        while count < max {
            at = step(at);
            if !same(at) {
                break;
            }
            count += 1;
        }
        count
    };
    let negative = |pos: BlockPos| along(pos, -1);
    let positive = |pos: BlockPos| along(pos, 1);
    let down = |pos: BlockPos| BlockPos::new(pos.x, pos.y - 1, pos.z);
    let up = |pos: BlockPos| BlockPos::new(pos.x, pos.y + 1, pos.z);

    let back = limit(block, &negative, LIMIT);
    let forth = limit(block, &positive, LIMIT);
    let centre = back as usize;
    // Each column's reach down (`min`) and up (`max`) from the row of `block`, no
    // farther than the column beside it towards `block` reaches.
    let mut bounds = vec![(0, 0); centre + 1 + forth as usize];
    bounds[centre] = (limit(block, &down, LIMIT), limit(block, &up, LIMIT));
    let centre_min = bounds[centre].0;
    for i in 1..=back {
        let previous = bounds[centre - (i as usize - 1)];
        let column = along(block, -i);
        bounds[centre - i as usize] = (
            limit(column, &down, previous.0),
            limit(column, &up, previous.1),
        );
    }
    for i in 1..=forth {
        let previous = bounds[centre + i as usize - 1];
        let column = along(block, i);
        bounds[centre + i as usize] = (
            limit(column, &down, previous.0),
            limit(column, &up, previous.1),
        );
    }

    let (mut best_start, mut best_row, mut best_width, mut best_height) = (0, 0, 0, 0);
    let mut heights = vec![0; bounds.len()];
    // Each row from the lowest the column of `block` reaches up to its own: the
    // columns' heights above the row, and the largest rectangle standing on it.
    let mut row = centre_min;
    while row >= 0 {
        for (height, &(min, max)) in heights.iter_mut().zip(&bounds) {
            let low = centre_min - min;
            let high = centre_min + max;
            *height = if row >= low && row <= high {
                high + 1 - row
            } else {
                0
            };
        }
        let ((start, end), height) = max_rectangle_location(&heights);
        let width = 1 + end - start;
        if width * height > best_width * best_height {
            best_start = start;
            best_row = row;
            best_width = width;
            best_height = height;
        }
        row -= 1;
    }
    let corner = along(block, best_start - back);
    FoundRectangle {
        min_corner: BlockPos::new(corner.x, corner.y + best_row - centre_min, corner.z),
        axis1_size: best_width,
        axis2_size: best_height,
    }
}

/// `BlockUtil.getMaxRectangleLocation` (BlockUtil.txt 541–621): the largest rectangle
/// under a histogram, by a stack of columns; its first and last column and its height.
/// Of two as large the first found is kept.
fn max_rectangle_location(heights: &[i32]) -> ((i32, i32), i32) {
    let (mut start, mut end, mut best) = (0_i32, 0_i32, 0_i32);
    let mut stack: Vec<usize> = vec![0];
    for i in 1..=heights.len() {
        let height = if i == heights.len() { 0 } else { heights[i] };
        while let Some(&top) = stack.last() {
            let top_height = heights[top];
            if height >= top_height {
                stack.push(i);
                break;
            }
            stack.pop();
            let left = stack.last().map_or(0, |&below| below + 1) as i32;
            if top_height * (i as i32 - left) > best * (end - start) {
                end = i as i32;
                start = left;
                best = top_height;
            }
        }
        if stack.is_empty() {
            stack.push(i);
        }
    }
    ((start, end - 1), best)
}

/// The value of a position on a horizontal axis, and on the other one.
fn on_axis(axis: PortalAxis, value: [f64; 3]) -> (f64, f64) {
    match axis {
        PortalAxis::X => (value[0], value[2]),
        PortalAxis::Z => (value[2], value[0]),
    }
}

fn block_on_axis(axis: PortalAxis, pos: BlockPos) -> (i32, i32) {
    match axis {
        PortalAxis::X => (pos.x, pos.z),
        PortalAxis::Z => (pos.z, pos.x),
    }
}

/// `Mth.clamp(double, double, double)`.
fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if value < low { low } else { value.min(high) }
}

/// `Mth.inverseLerp`.
fn inverse_lerp(value: f64, start: f64, end: f64) -> f64 {
    (value - start) / (end - start)
}

/// `PortalShape.getRelativePosition` (PortalShape.txt 504–603): where an entity of
/// `entity_size` (its width and height, floats as the game keeps them) at `position`
/// stands in the portal `rectangle` along `axis`. Along the axis, the part of the room
/// the portal leaves it, from 0 to 1 (0.5 where it has none); up, the same from the
/// bottom (0 where it has none); across, its distance from the portal's middle plane,
/// which is not held.
pub fn relative_position(
    rectangle: &FoundRectangle,
    axis: PortalAxis,
    position: [f64; 3],
    entity_size: (f32, f32),
) -> [f64; 3] {
    let width = f64::from(entity_size.0);
    let height = f64::from(entity_size.1);
    let room_along = f64::from(rectangle.axis1_size) - width;
    let room_up = f64::from(rectangle.axis2_size) - height;
    let corner = rectangle.min_corner;
    let (corner_along, corner_across) = block_on_axis(axis, corner);
    let (along, across) = on_axis(axis, position);
    let relative_along = if room_along > 0.0 {
        let start = f64::from(corner_along) + width / 2.0;
        clamp(inverse_lerp(along - start, 0.0, room_along), 0.0, 1.0)
    } else {
        0.5
    };
    let relative_up = if room_up > 0.0 {
        clamp(
            inverse_lerp(position[1] - f64::from(corner.y), 0.0, room_up),
            0.0,
            1.0,
        )
    } else {
        0.0
    };
    let relative_across = across - (f64::from(corner_across) + 0.5);
    [relative_along, relative_up, relative_across]
}

/// The first half of `NetherPortalBlock.getDimensionTransitionFromExit`
/// (NetherPortalBlock.txt 332–380): the axis of the portal block `block` an entity
/// leaves by, and its place in that portal's rectangle. A block that is not a portal
/// (the game asks for `HORIZONTAL_AXIS`, which only the portal has), or that the
/// reader cannot read, gives the X axis and (0.5, 0, 0): the middle, at the bottom, in
/// the plane.
pub fn exit_relative(
    block: BlockPos,
    position: [f64; 3],
    entity_size: (f32, f32),
    read: BlockReader,
) -> (PortalAxis, [f64; 3]) {
    match read(block) {
        Some(state) if is_block(state, blocks::NETHER_PORTAL) => {
            let axis = crate::portal::axis_of(state);
            let rectangle = largest_rectangle(block, axis, read);
            (
                axis,
                relative_position(&rectangle, axis, position, entity_size),
            )
        }
        _ => (PortalAxis::X, [0.5, 0.0, 0.0]),
    }
}

/// `NetherPortalBlock.createDimensionTransition` (NetherPortalBlock.txt 382–508) before
/// its collision step: where an entity of `entity_size` arrives in the portal
/// `rectangle` (whose blocks lie along `axis`, X where the corner is no portal), at
/// `relative` as [`relative_position`] gave it in the portal it left along
/// `source_axis`, and its velocity and yaw after.
///
/// The transition is made with no velocity and a yaw of 0, or 90 where the two axes
/// differ, both relative (`Relative.DELTA` and `Relative.ROTATION`, 209–247), which
/// `PositionMoveRotation.calculateAbsolute` applies: the yaw is turned by 90 degrees,
/// and since `Relative.DELTA` holds `ROTATE_DELTA` the velocity is turned with it, by
/// `Vec3.yRot` of the old yaw less the new, -90 degrees, whose `Mth.sin` and `Mth.cos`
/// the game's table gives as -1 and 0 exactly (read from the 26.3 jar's
/// `PositionMoveRotation` and `Relative`, 2026-10-30). The pitch is not changed, so its
/// turn is none.
pub fn exit_pose(
    rectangle: &FoundRectangle,
    axis: PortalAxis,
    relative: [f64; 3],
    source_axis: PortalAxis,
    entity_size: (f32, f32),
    velocity: [f64; 3],
    yaw: f32,
) -> ([f64; 3], [f64; 3], f32) {
    let width = f64::from(entity_size.0);
    let height = f64::from(entity_size.1);
    let turn: f32 = if axis == source_axis { 0.0 } else { 90.0 };
    let along = width / 2.0 + (f64::from(rectangle.axis1_size) - width) * relative[0];
    let up = (f64::from(rectangle.axis2_size) - height) * relative[1];
    let across = 0.5 + relative[2];
    let corner = rectangle.min_corner;
    let (x, z) = match axis {
        PortalAxis::X => (along, across),
        PortalAxis::Z => (across, along),
    };
    let position = [
        f64::from(corner.x) + x,
        f64::from(corner.y) + up,
        f64::from(corner.z) + z,
    ];
    let velocity = if axis == source_axis {
        velocity
    } else {
        // `Vec3.yRot(-π/2)`, as the game computes it.
        let (cos, sin) = (0.0_f64, -1.0_f64);
        [
            velocity[0] * cos + velocity[2] * sin,
            velocity[1],
            velocity[2] * cos - velocity[0] * sin,
        ]
    };
    (position, velocity, yaw + turn)
}

/// A box: its low corner and its high one, x, y, z.
type Box3 = [f64; 6];

/// `PortalShape.findCollisionFreePosition` (PortalShape.txt 605–672) and
/// `CollisionGetter.findFreePosition` (CollisionGetter.txt 395–489): the place nearest
/// `position` where an entity of `entity_size` is clear of every block, looking no
/// more than one block up and moving the entity's centre no farther than the box it
/// stands in; `position` itself where there is none, or for an entity wider or taller
/// than 4.
///
/// The free places are a box of the entity's width about its centre, from its centre
/// up one block, less every block's collision boxes grown by half the entity's size;
/// the answer is the point of them nearest the centre, lowered back to the feet.
///
/// Blocks are read as `BlockCollisions` reads them (as `clustine-sim`'s physics does):
/// within the box grown by the entity's size, and the cells beyond it only for a
/// shape that reaches out of its block; a block whose shape lies beyond the border is
/// not counted. A block the reader cannot read collides with nothing, as the game
/// treats a chunk it does not have. Shapes are those the empty context has: powder
/// snow and scaffolding, which answer each entity otherwise, are taken as the table
/// has them.
pub fn free_position(
    position: [f64; 3],
    entity_size: (f32, f32),
    read: BlockReader,
    border: f64,
) -> [f64; 3] {
    if entity_size.0 > 4.0 || entity_size.1 > 4.0 {
        return position;
    }
    let width = f64::from(entity_size.0);
    let height = f64::from(entity_size.1);
    let half_height = height / 2.0;
    let centre = [position[0], position[1] + half_height, position[2]];
    // `AABB.ofSize(centre, width, 0, width).expandTowards(0, 1, 0).inflate(1e-6)`.
    let inflate = 1.0e-6;
    let room: Box3 = [
        centre[0] - width / 2.0 - inflate,
        centre[1] - inflate,
        centre[2] - width / 2.0 - inflate,
        centre[0] + width / 2.0 + inflate,
        centre[1] + 1.0 + inflate,
        centre[2] + width / 2.0 + inflate,
    ];
    // `Shapes.create` makes nothing of a box thinner than 1e-7, which a box of a
    // width of 0 is not once inflated; this is the game's first test all the same.
    if (0..3).any(|axis| room[axis + 3] - room[axis] < 1.0e-7) {
        return position;
    }
    let search: Box3 = [
        room[0] - width,
        room[1] - height,
        room[2] - width,
        room[3] + width,
        room[4] + height,
        room[5] + width,
    ];
    let mut obstacles: Vec<Box3> = Vec::new();
    for boxes in block_collisions(&search, read) {
        let bounds = bounds_of(&boxes);
        // `WorldBorder.isWithinBounds(AABB)`: its high edges less 1e-5 (a float's).
        let margin = f64::from(1.0e-5_f32);
        if !within_border_at(border, bounds[0], bounds[2])
            || !within_border_at(border, bounds[3] - margin, bounds[5] - margin)
        {
            continue;
        }
        for shape_box in boxes {
            obstacles.push([
                shape_box[0] - width / 2.0,
                shape_box[1] - half_height,
                shape_box[2] - width / 2.0,
                shape_box[3] + width / 2.0,
                shape_box[4] + half_height,
                shape_box[5] + width / 2.0,
            ]);
        }
    }
    match closest_free_point(&room, &obstacles, centre) {
        Some(point) => [point[0], point[1] - half_height, point[2]],
        None => position,
    }
}

/// The smallest box about all of `boxes` (`VoxelShape.bounds`).
fn bounds_of(boxes: &[Box3]) -> Box3 {
    let mut bounds = boxes[0];
    for shape_box in &boxes[1..] {
        for axis in 0..3 {
            bounds[axis] = bounds[axis].min(shape_box[axis]);
            bounds[axis + 3] = bounds[axis + 3].max(shape_box[axis + 3]);
        }
    }
    bounds
}

/// `BlockCollisions` over `search` with the empty context: for each block whose
/// collision shape meets the box (more than touching it), the shape's boxes where they
/// are in the world, x fastest, then y, then z.
fn block_collisions(search: &Box3, read: BlockReader) -> Vec<Vec<Box3>> {
    let low: [i32; 3] = std::array::from_fn(|axis| (search[axis] - 1.0e-7).floor() as i32 - 1);
    let high: [i32; 3] = std::array::from_fn(|axis| (search[axis + 3] + 1.0e-7).floor() as i32 + 1);
    let mut found = Vec::new();
    for z in low[2]..=high[2] {
        for y in low[1]..=high[1] {
            for x in low[0]..=high[0] {
                let edges = [x, y, z]
                    .into_iter()
                    .enumerate()
                    .filter(|&(axis, value)| value == low[axis] || value == high[axis])
                    .count();
                if edges == 3 {
                    continue;
                }
                let pos = BlockPos::new(x, y, z);
                let Some(state) = read(pos) else {
                    continue;
                };
                // A cell at one edge counts only for a shape that reaches out of its
                // block; one at two only for a moving piston, whose shape is its block
                // entity's and which the table gives none.
                if edges == 1 && !state.has_large_collision_shape() {
                    continue;
                }
                if edges == 2 {
                    continue;
                }
                let shape = state.props().collision_shape();
                let shift = offset_of(state, pos);
                let boxes: Vec<Box3> = shape
                    .boxes()
                    .map(|b| {
                        [
                            b[0] + f64::from(x) + shift[0],
                            b[1] + f64::from(y) + shift[1],
                            b[2] + f64::from(z) + shift[2],
                            b[3] + f64::from(x) + shift[0],
                            b[4] + f64::from(y) + shift[1],
                            b[5] + f64::from(z) + shift[2],
                        ]
                    })
                    .collect();
                if boxes.iter().any(|b| overlaps(b, search)) {
                    found.push(boxes);
                }
            }
        }
    }
    found
}

/// Whether two boxes share more than a face.
fn overlaps(a: &Box3, b: &Box3) -> bool {
    (0..3).all(|axis| a[axis] < b[axis + 3] && a[axis + 3] > b[axis])
}

/// `BlockStateBase.getOffset` for a block whose shape the game shifts by where it
/// stands (bamboo, pointed dripstone), from `Mth.getSeed(x, 0, z)`; nothing for the
/// rest.
fn offset_of(state: BlockState, pos: BlockPos) -> [f64; 3] {
    let Some(physics) = state.physics() else {
        return [0.0; 3];
    };
    if physics.offset_type == OffsetType::None {
        return [0.0; 3];
    }
    let seed = get_seed(pos.x, 0, pos.z);
    let limit = f64::from(physics.max_horizontal_offset);
    let part = |shift: i64| f64::from(((seed >> shift) & 15) as f32 / 15.0) - 0.5;
    let x = clamp(part(0) * 0.5, -limit, limit);
    let z = clamp(part(8) * 0.5, -limit, limit);
    let y = match physics.offset_type {
        OffsetType::Xyz => {
            (f64::from(((seed >> 4) & 15) as f32 / 15.0) - 1.0)
                * f64::from(physics.max_vertical_offset)
        }
        _ => 0.0,
    };
    [x, y, z]
}

/// `Mth.getSeed(x, y, z)`.
fn get_seed(x: i32, y: i32, z: i32) -> i64 {
    let mixed = i64::from(x.wrapping_mul(3_129_871))
        ^ (i64::from(z).wrapping_mul(116_129_781_i64))
        ^ i64::from(y);
    let mixed = mixed
        .wrapping_mul(mixed)
        .wrapping_mul(42_317_861_i64)
        .wrapping_add(mixed.wrapping_mul(11));
    mixed >> 16
}

/// `Shapes.join(room, obstacles, ONLY_FIRST).closestPointTo(centre)`: the point of
/// `room` outside every obstacle (their faces count as outside) nearest `centre`,
/// or none where the obstacles fill it.
///
/// The room is cut into cells at every obstacle's face within it, as the game's shapes
/// are; a cell is free where no obstacle holds its middle. Each free cell offers the
/// point of it nearest the centre, and the nearest is kept, the first of equals in the
/// order x, then y, then z. The game merges free cells into boxes before it looks
/// (`VoxelShape.forAllBoxes`), so of two points exactly as near it may keep the other.
fn closest_free_point(room: &Box3, obstacles: &[Box3], centre: [f64; 3]) -> Option<[f64; 3]> {
    let cuts: [Vec<f64>; 3] = std::array::from_fn(|axis| {
        let (low, high) = (room[axis], room[axis + 3]);
        let mut cuts = vec![low, high];
        for obstacle in obstacles {
            for value in [obstacle[axis], obstacle[axis + 3]] {
                if value > low && value < high {
                    cuts.push(value);
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        cuts
    });
    let mut best: Option<(f64, [f64; 3])> = None;
    for xs in cuts[0].windows(2) {
        for ys in cuts[1].windows(2) {
            for zs in cuts[2].windows(2) {
                let cell = [xs[0], ys[0], zs[0], xs[1], ys[1], zs[1]];
                let middle: [f64; 3] =
                    std::array::from_fn(|axis| (cell[axis] + cell[axis + 3]) / 2.0);
                let held = obstacles.iter().any(|obstacle| {
                    (0..3).all(|axis| {
                        obstacle[axis] < middle[axis] && middle[axis] < obstacle[axis + 3]
                    })
                });
                if held {
                    continue;
                }
                let point: [f64; 3] =
                    std::array::from_fn(|axis| clamp(centre[axis], cell[axis], cell[axis + 3]));
                let distance = (0..3)
                    .map(|axis| (centre[axis] - point[axis]).powi(2))
                    .sum::<f64>();
                if best.is_none_or(|(nearest, _)| distance < nearest) {
                    best = Some((distance, point));
                }
            }
        }
    }
    best.map(|(_, point)| point)
}

/// A horizontal step: east or south along an axis (`Direction.get(POSITIVE, axis)`),
/// and its `getClockWise`.
fn positive_step(axis: PortalAxis) -> (i32, i32) {
    match axis {
        PortalAxis::X => (1, 0),
        PortalAxis::Z => (0, 1),
    }
}

/// `Direction.getClockWise` of a horizontal step: east turns south, south west.
fn clockwise(step: (i32, i32)) -> (i32, i32) {
    (-step.1, step.0)
}

/// `BlockPos.spiralAround(centre, radius, first, second)` (BlockPos.txt 1054–1073 and
/// its iterator, `BlockPos$6`): the columns of the square of `radius` about `centre`,
/// from the centre outwards, legs of 1, 1, 2, 2, 3, 3 ... blocks long turning from
/// `first` to `second` to their opposites. Steps are (x, z).
fn spiral_around(
    centre: BlockPos,
    radius: i32,
    first: (i32, i32),
    second: (i32, i32),
) -> Vec<BlockPos> {
    let directions = [first, second, (-first.0, -first.1), (-second.0, -second.1)];
    let legs = 4 * radius;
    let mut leg: i32 = -1;
    let mut leg_size = 0;
    let mut leg_index = 0;
    let mut last = BlockPos::new(centre.x + second.0, centre.y, centre.z + second.1);
    let mut columns = Vec::new();
    loop {
        let step = directions[((leg + 4) % 4) as usize];
        let cursor = BlockPos::new(last.x + step.0, last.y, last.z + step.1);
        last = cursor;
        if leg_index >= leg_size {
            if leg >= legs {
                return columns;
            }
            leg += 1;
            leg_index = 0;
            leg_size = leg / 2 + 1;
        }
        leg_index += 1;
        columns.push(cursor);
    }
}

/// What the search of `createPortal` reads, which remembers whether it met a block it
/// could not read.
struct Search<'a> {
    read: BlockReader<'a>,
    unreadable: bool,
}

impl Search<'_> {
    fn state(&mut self, pos: BlockPos) -> Option<BlockState> {
        let state = (self.read)(pos);
        if state.is_none() {
            self.unreadable = true;
        }
        state
    }

    /// `PortalForcer.canPortalReplaceBlock` (PortalForcer.txt 491–508): what placing
    /// may overwrite (`canBeReplaced()`), and holding no fluid.
    fn replaceable(&mut self, pos: BlockPos) -> bool {
        self.state(pos)
            .is_some_and(|state| state.props().can_be_replaced() && state.props().fluid().is_none())
    }

    /// `PortalForcer.canHostFrame` (PortalForcer.txt 510–571): whether a frame along
    /// `step` from `origin`, moved `offset` across it, stands on solid blocks
    /// (`isSolid`) 4 long and has room above them 4 high, read along first and up, row
    /// by row, and given up at the first that fails.
    fn can_host_frame(&mut self, origin: BlockPos, step: (i32, i32), offset: i32) -> bool {
        let across = clockwise(step);
        for along in -1..3 {
            for up in -1..4 {
                let pos = BlockPos::new(
                    origin.x + step.0 * along + across.0 * offset,
                    origin.y + up,
                    origin.z + step.1 * along + across.1 * offset,
                );
                if up < 0 {
                    if !self
                        .state(pos)
                        .is_some_and(|state| state.props().is_solid())
                    {
                        return false;
                    }
                } else if !self.replaceable(pos) {
                    return false;
                }
            }
        }
        true
    }
}

/// What `createPortal` sets, in its order, the rectangle of the portal's blocks and
/// their axis.
pub type MadePortal = (Vec<(BlockPos, BlockState)>, FoundRectangle, PortalAxis);

/// `PortalForcer.createPortal(target, axis)` (PortalForcer.txt 83–489): a portal made
/// near `target` for a traveller that came through a portal along `axis` (X where its
/// block had none), in a level whose lowest block is `min_y` and whose top is
/// `min_y + logical_height − 1`, within a border of half-width `border`.
///
/// It looks over the 33 × 33 columns about the target, spiralling out, from the top
/// (or the heightmap, if lower) down, for a block that can be replaced, on top of
/// those below it that can be too, with 4 blocks of room above its floor, where a
/// frame 4 long would stand on solid ground with room for it above: the nearest
/// (`distSqr`) such spot with the same room on both sides of it (a three-deep spot),
/// else the nearest with room only in its own plane (a one-deep one). A gap of 1 or 2
/// blocks is not taken; a column outside the border, or whose step along the axis is,
/// is not looked at. Where it finds none, it makes a platform of obsidian 2 × 3 with 3
/// blocks of air over it at the target's column, less one step along the axis, at the
/// target's height held between 70 (or the floor's next block) and 9 below the top,
/// within the border; it fails only where those two limits leave no room.
///
/// Then it frames the spot with obsidian 4 wide and 5 high and fills the inside, 2 by
/// 3, with portal along `axis`. The blocks are given in the order the game sets them:
/// the platform's (`setBlockAndUpdate`), the frame's (`setBlockAndUpdate`), then the
/// portal's (`setBlock` with flags 18: to clients, with no neighbour's shape updated).
///
/// The top is taken as `min_y + logical_height − 1`, which the game holds below
/// `getMaxY` too: the dimension type's codec keeps the logical height within the
/// height. A column whose height the caller cannot read, or one in which the search
/// reads a block the caller cannot, is unsuitable: every spot in it is passed over.
pub fn make_portal(
    target: BlockPos,
    axis: PortalAxis,
    read: BlockReader,
    heights: HeightReader,
    min_y: i32,
    logical_height: i32,
    border: f64,
) -> Option<MadePortal> {
    let step = positive_step(axis);
    let top = min_y + logical_height - 1;
    let mut three_deep: Option<(f64, BlockPos)> = None;
    let mut one_deep: Option<(f64, BlockPos)> = None;
    for column in spiral_around(target, 16, (1, 0), (0, 1)) {
        let (x, z) = (column.x, column.z);
        let Some(height) = heights(x, z) else {
            continue;
        };
        let start_y = top.min(height);
        if !within_border(border, x, z) || !within_border(border, x + step.0, z + step.1) {
            continue;
        }
        let mut search = Search {
            read,
            unreadable: false,
        };
        // The spots this column offers, in the order the game meets them, which are
        // weighed against the others only if nothing in the column was unreadable.
        let mut offers: Vec<(BlockPos, f64, bool)> = Vec::new();
        let mut y = start_y;
        while y >= min_y {
            if !search.replaceable(BlockPos::new(x, y, z)) {
                y -= 1;
                continue;
            }
            let start = y;
            while y > min_y && search.replaceable(BlockPos::new(x, y - 1, z)) {
                y -= 1;
            }
            let gap = start - y;
            if y + 4 > top || (gap > 0 && gap < 3) {
                y -= 1;
                continue;
            }
            let spot = BlockPos::new(x, y, z);
            if search.can_host_frame(spot, step, 0) {
                let distance = dist_sqr(target, spot);
                let both_sides =
                    search.can_host_frame(spot, step, -1) && search.can_host_frame(spot, step, 1);
                offers.push((spot, distance, both_sides));
            }
            y -= 1;
        }
        if search.unreadable {
            continue;
        }
        for (spot, distance, both_sides) in offers {
            if both_sides && three_deep.is_none_or(|(best, _)| best > distance) {
                three_deep = Some((distance, spot));
            }
            if three_deep.is_none() && one_deep.is_none_or(|(best, _)| best > distance) {
                one_deep = Some((distance, spot));
            }
        }
    }

    let mut changes = Vec::new();
    let obsidian = blocks::OBSIDIAN;
    let spot = match three_deep.or(one_deep) {
        Some((_, spot)) => spot,
        None => {
            let floor = (min_y + 1).max(70);
            let ceiling = top - 9;
            if ceiling < floor {
                return None;
            }
            let spot = clamp_to_border(
                border,
                BlockPos::new(
                    target.x - step.0,
                    target.y.clamp(floor, ceiling),
                    target.z - step.1,
                ),
            );
            let across = clockwise(step);
            for side in -1..2 {
                for along in 0..2 {
                    for up in -1..3 {
                        let state = if up < 0 { obsidian } else { blocks::AIR };
                        let pos = BlockPos::new(
                            spot.x + along * step.0 + side * across.0,
                            spot.y + up,
                            spot.z + along * step.1 + side * across.1,
                        );
                        changes.push((pos, state));
                    }
                }
            }
            spot
        }
    };
    for along in -1..3 {
        for up in -1..4 {
            if along == -1 || along == 2 || up == -1 || up == 3 {
                let pos = BlockPos::new(
                    spot.x + along * step.0,
                    spot.y + up,
                    spot.z + along * step.1,
                );
                changes.push((pos, obsidian));
            }
        }
    }
    let portal = set(blocks::NETHER_PORTAL, "axis", axis_name(axis));
    for along in 0..2 {
        for up in 0..3 {
            let pos = BlockPos::new(
                spot.x + along * step.0,
                spot.y + up,
                spot.z + along * step.1,
            );
            changes.push((pos, portal));
        }
    }
    let rectangle = FoundRectangle {
        min_corner: spot,
        axis1_size: 2,
        axis2_size: 3,
    };
    Some((changes, rectangle, axis))
}

#[cfg(test)]
#[path = "portal_forcer_tests.rs"]
mod tests;
