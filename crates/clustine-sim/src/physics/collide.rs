//! The collider: the shapes of the blocks a box may meet (`BlockCollisions`, in the
//! game's cursor order), cutting a move down by them (`Entity.collideWithShapes`,
//! `Shapes.collide`), the block an entity stands on (`findSupportingBlock`) and
//! whether a box is free (`noCollision`) (`docs/groundwork/n4-brief.md`, sections 1.3
//! to 1.5).
//!
//! No entity collides with these entities yet (boats, minecarts and shulkers come
//! later), and the world border's shape is left out: Clustine has none, and the game's
//! default lies near 30 million blocks out.

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, BlockState, OffsetType, blocks};
use clustine_world::BlockPos;

use super::aabb::Aabb;
use super::voxel::{Cells, Placed};
use super::{Vec3, World};

/// `PowderSnowBlock.FALLING_COLLISION_SHAPE`: what powder snow is to something falling
/// into it from more than two and a half blocks.
const POWDER_SNOW_FALLING: [f64; 6] = [0.0, 0.0, 0.0, 1.0, 0.899_999_976_158_142_1, 1.0];

/// `ScaffoldingBlock.SHAPE_UNSTABLE_BOTTOM`: the bottom of scaffolding that stands on
/// nothing, met from below its top.
const SCAFFOLDING_BOTTOM: [f64; 6] = [0.0, 0.0, 0.0, 1.0, 0.125, 1.0];

/// What a block's collision shape depends on of the entity that meets it (the game's
/// `EntityCollisionContext` of an entity that is not living: it never descends and
/// holds nothing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Context {
    /// `entityBottom`: the entity's y where the context was made.
    pub(crate) bottom: f64,
    pub(crate) fall_distance: f64,
    /// Whether the entity is a falling block, which powder snow holds up.
    pub(crate) falling_block: bool,
}

impl Context {
    /// `isAbove(shape, pos, _)` for a shape whose top is `top` within its block.
    fn is_above(self, pos: BlockPos, top: f64) -> bool {
        self.bottom > f64::from(pos.y) + top - f64::from(1.0e-5_f32)
    }
}

/// A shape the collider found, at the block it is of.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Found {
    pub(crate) pos: BlockPos,
    pub(crate) shape: Placed,
}

/// `BlockStateBase.getOffset` for a block whose shape shifts by its position.
pub(crate) fn offset_of(state: BlockState, pos: BlockPos) -> Option<Vec3> {
    let physics = state.physics()?;
    if physics.offset_type == OffsetType::None {
        return None;
    }
    let seed = clustine_noise::random::get_seed(pos.x, 0, pos.z);
    let limit = f64::from(physics.max_horizontal_offset);
    let part = |shift: i64| f64::from(((seed >> shift) & 15) as f32 / 15.0) - 0.5;
    let x = (part(0) * 0.5).clamp(-limit, limit);
    let z = (part(8) * 0.5).clamp(-limit, limit);
    let y = match physics.offset_type {
        OffsetType::Xyz => {
            (f64::from(((seed >> 4) & 15) as f32 / 15.0) - 1.0)
                * f64::from(physics.max_vertical_offset)
        }
        _ => 0.0,
    };
    Some([x, y, z])
}

/// `context.getCollisionShape(state, level, pos)` for the entity of `context`, placed
/// at `pos`, and whether it is the full block instance, which the game yields without
/// testing its cells; `None` where it is empty.
pub(crate) fn collision_shape(
    state: BlockState,
    pos: BlockPos,
    context: Context,
) -> Option<(Placed, bool)> {
    let at = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
    let placed = |cells| Placed {
        cells,
        shift: None,
        offset: at,
    };
    if is_class(state, BlockClass::PowderSnowBlock) {
        if context.fall_distance > 2.5 {
            return Some((placed(Cells::Box(POWDER_SNOW_FALLING)), false));
        }
        if context.falling_block {
            let block = blocks::STONE.voxel_collision_shape();
            return Some((placed(Cells::Data(block)), true));
        }
        // An item is not among `powder_snow_walkable_mobs`.
        return None;
    }
    if is_class(state, BlockClass::ScaffoldingBlock) {
        // The table holds the stable shape, the empty context's answer.
        if context.is_above(pos, 1.0) {
            let shape = state.voxel_collision_shape();
            return Some((placed(Cells::Data(shape)), shape.is_block()));
        }
        let distance = clustine_blocks::data::number(state, "distance").unwrap_or(0);
        if distance != 0
            && clustine_blocks::data::flag(state, "bottom")
            && context.is_above(pos, 0.0)
        {
            return Some((placed(Cells::Box(SCAFFOLDING_BOTTOM)), false));
        }
        return None;
    }
    let shape = state.voxel_collision_shape();
    if shape.is_empty() {
        return None;
    }
    Some((
        Placed {
            cells: Cells::Data(shape),
            shift: offset_of(state, pos),
            offset: at,
        },
        shape.is_block(),
    ))
}

/// `BlockCollisions` over `bounds`: the shapes of the cells from a block beyond its
/// edges, x fastest, then y, then z; a cell at an edge is looked at only for a shape
/// that reaches out of its block, one at two edges only for a moving piston, one at
/// three never; a shape is kept where it overlaps the box. A chunk the world does not
/// have gives none.
pub(crate) fn block_collisions<W: World + ?Sized>(
    world: &W,
    context: Context,
    bounds: &Aabb,
) -> Vec<Found> {
    let low: [i32; 3] = std::array::from_fn(|axis| super::floor(bounds.min[axis] - 1.0e-7) - 1);
    let high: [i32; 3] = std::array::from_fn(|axis| super::floor(bounds.max[axis] + 1.0e-7) + 1);
    let mut found = Vec::new();
    for z in low[2]..=high[2] {
        for y in low[1]..=high[1] {
            for x in low[0]..=high[0] {
                let edges = [(x, 0), (y, 1), (z, 2)]
                    .into_iter()
                    .filter(|(value, axis)| *value == low[*axis] || *value == high[*axis])
                    .count();
                if edges == 3 {
                    continue;
                }
                let pos = BlockPos::new(x, y, z);
                let Some(state) = world.block(pos) else {
                    continue;
                };
                if edges == 1 && !state.has_large_collision_shape() {
                    continue;
                }
                if edges == 2 && state != blocks::MOVING_PISTON {
                    continue;
                }
                let Some((shape, is_block)) = collision_shape(state, pos, context) else {
                    continue;
                };
                let kept = if is_block {
                    bounds.intersects(Aabb::of_block(pos))
                } else {
                    shape.overlaps_box(bounds)
                };
                if kept {
                    found.push(Found { pos, shape });
                }
            }
        }
    }
    found
}

/// `Direction.axisStepOrder`: y first, then the larger of x and z by size, x where
/// they are equal.
pub(crate) fn axis_step_order(vector: Vec3) -> [usize; 3] {
    if vector[0].abs() < vector[2].abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    }
}

/// `Shapes.collide(axis, box, shapes, d)`.
fn collide_along(axis: usize, bounds: &Aabb, shapes: &[Found], mut d: f64) -> f64 {
    for found in shapes {
        if d.abs() < 1.0e-7 {
            return 0.0;
        }
        d = found.shape.collide(axis, bounds, d);
    }
    d
}

/// `Entity.collideWithShapes`: `vector` cut down axis by axis, in the order of
/// [`axis_step_order`], by `shapes` in their order.
pub(crate) fn collide_with_shapes(vector: Vec3, bounds: &Aabb, shapes: &[Found]) -> Vec3 {
    if shapes.is_empty() {
        return vector;
    }
    let mut moved = [0.0; 3];
    for axis in axis_step_order(vector) {
        if vector[axis] == 0.0 {
            continue;
        }
        let distance = collide_along(axis, &bounds.moved(moved), shapes, vector[axis]);
        moved[axis] = distance;
    }
    moved
}

/// `Entity.collide` for an entity that steps up nothing and that no entity collides
/// with: the move the blocks leave of `vector` for a box at `bounds`.
pub(crate) fn collide<W: World + ?Sized>(
    world: &W,
    context: Context,
    bounds: &Aabb,
    vector: Vec3,
) -> Vec3 {
    if super::length_sqr(vector) == 0.0 {
        return vector;
    }
    let shapes = block_collisions(world, context, &bounds.expand_towards(vector));
    collide_with_shapes(vector, bounds, &shapes)
}

/// `CollisionGetter.findSupportingBlock`: of the blocks whose shapes `probe` meets, the
/// one whose centre is nearest `position`, the greater by the game's order of positions
/// (y, then z, then x) where two are as near.
pub(crate) fn find_supporting_block<W: World + ?Sized>(
    world: &W,
    context: Context,
    probe: &Aabb,
    position: Vec3,
) -> Option<BlockPos> {
    let mut best: Option<BlockPos> = None;
    let mut nearest = f64::MAX;
    for found in block_collisions(world, context, probe) {
        let pos = found.pos;
        // `Vec3i.distToCenterSqr`: from the low corner of a point half a block lower.
        let dx = f64::from(pos.x) - (position[0] - 0.5);
        let dy = f64::from(pos.y) - (position[1] - 0.5);
        let dz = f64::from(pos.z) - (position[2] - 0.5);
        let distance = dx * dx + dy * dy + dz * dz;
        let order = |pos: BlockPos| (pos.y, pos.z, pos.x);
        if distance < nearest
            || distance == nearest && best.is_none_or(|best| order(best) < order(pos))
        {
            best = Some(pos);
            nearest = distance;
        }
    }
    best
}

/// `CollisionGetter.noCollision(entity, bounds)`: no block's shape meets the box.
pub(crate) fn no_collision<W: World + ?Sized>(world: &W, context: Context, bounds: &Aabb) -> bool {
    block_collisions(world, context, bounds).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_goes_before_z_unless_z_is_the_longer() {
        assert_eq!(axis_step_order([0.1, -1.0, 0.1]), [1, 0, 2]);
        assert_eq!(axis_step_order([0.1, -1.0, -0.2]), [1, 2, 0]);
    }
}
