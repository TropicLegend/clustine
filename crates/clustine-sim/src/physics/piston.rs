//! What a moving piston is to an entity (`PistonMovingBlockEntity`, `PistonMath` and
//! `Entity.limitPistonMovement` of the 26.3 jar): the shape it collides with as its
//! block goes, the boxes it pushes entities with, and the limit on how far pistons may
//! move one entity along an axis in one tick.
//!
//! A move by a piston passes the direction it pushes in to the collider
//! ([`Context::noclip`](super::collide::Context)), as the game's thread-local `NOCLIP`
//! does: a moving block on its way the same way collides with nothing but the base of a
//! retracting piston, so that what it pushes does not stop against it.

use clustine_blocks::data::is_block;
use clustine_data::{BlockState, Face, blocks};
use clustine_world::BlockPos;

use super::Vec3;
use super::aabb::Aabb;
use super::voxel::{Cells, Placed};

/// `PistonMovingBlockEntity.PUSH_OFFSET`: how much further than the block an entity is
/// pushed.
pub(crate) const PUSH_OFFSET: f64 = 0.01;

/// `Entity.applyPistonMovementRestriction`'s bound: at most this far along an axis in a
/// tick, from all the pistons together.
const TICK_MOVEMENT: f64 = 0.51;

/// A moving piston's block entity as the collider and the pushes read it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MovingBlock {
    pub(crate) moved: BlockState,
    /// The piston's facing (`direction`).
    pub(crate) direction: Face,
    pub(crate) extending: bool,
    pub(crate) source: bool,
    /// `progress`, 0, 0.5 or 1.
    pub(crate) progress: f32,
}

/// `Direction.getStepX`, `getStepY` and `getStepZ`.
pub(crate) fn step(face: Face) -> [i32; 3] {
    match face {
        Face::Down => [0, -1, 0],
        Face::Up => [0, 1, 0],
        Face::North => [0, 0, -1],
        Face::South => [0, 0, 1],
        Face::West => [-1, 0, 0],
        Face::East => [1, 0, 0],
    }
}

/// The axis of a face: 0, 1 or 2 for x, y and z.
pub(crate) fn axis(face: Face) -> usize {
    match face {
        Face::West | Face::East => 0,
        Face::Down | Face::Up => 1,
        Face::North | Face::South => 2,
    }
}

/// The name a face has as a property's value.
fn face_name(face: Face) -> &'static str {
    match face {
        Face::Down => "down",
        Face::Up => "up",
        Face::North => "north",
        Face::South => "south",
        Face::West => "west",
        Face::East => "east",
    }
}

impl MovingBlock {
    /// `getMovementDirection`: the way the block goes, back towards the piston as it
    /// retracts.
    pub(crate) fn movement_direction(&self) -> Face {
        if self.extending {
            self.direction
        } else {
            self.direction.opposite()
        }
    }

    /// `getExtendedProgress(progress)`: how far from where it stands the block is
    /// drawn, along the piston's facing, as a float.
    fn extended_progress(&self, progress: f32) -> f32 {
        if self.extending {
            progress - 1.0
        } else {
            1.0 - progress
        }
    }

    /// The offset of `moveByPositionAndProgress`: the block's position plus its
    /// extended progress along the facing, each in double as the game adds them.
    pub(crate) fn offset(&self, pos: BlockPos) -> Vec3 {
        let along = f64::from(self.extended_progress(self.progress));
        let [x, y, z] = step(self.direction);
        [
            f64::from(pos.x) + along * f64::from(x),
            f64::from(pos.y) + along * f64::from(y),
            f64::from(pos.z) + along * f64::from(z),
        ]
    }

    /// `getCollisionRelatedBlockState`: what pushes entities. A retracting piston's
    /// base pushes with its head, short once it is more than a quarter in.
    pub(crate) fn collision_related_state(&self) -> BlockState {
        if !self.extending && self.source && clustine_blocks::piston::is_piston_base(self.moved) {
            let piston_type = if is_block(self.moved, blocks::STICKY_PISTON) {
                "sticky"
            } else {
                "normal"
            };
            let facing = self.moved.property("facing").unwrap_or("north");
            return head(facing, self.progress > 0.25, piston_type);
        }
        self.moved
    }

    /// `PistonMovingBlockEntity.getCollisionShape`, placed at `pos`: the base of a
    /// retracting piston where it stands, and the moved block (a source's head) where
    /// its progress has taken it; only the base while the entity asking is being moved
    /// the way the block goes and the move is not done.
    pub(crate) fn collision_shapes(&self, pos: BlockPos, noclip: Option<Face>) -> Vec<Placed> {
        let at = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
        let mut shapes = Vec::new();
        if !self.extending && self.source && clustine_blocks::piston::is_piston_base(self.moved) {
            let base = self
                .moved
                .with_property("extended", "true")
                .unwrap_or(self.moved)
                .voxel_collision_shape();
            if !base.is_empty() {
                shapes.push(Placed {
                    cells: Cells::Data(base),
                    shift: None,
                    offset: at,
                });
            }
        }
        if f64::from(self.progress) < 1.0 && noclip == Some(self.movement_direction()) {
            return shapes;
        }
        let moved = if self.source {
            let short = self.extending != (1.0 - self.progress < 0.25);
            head(face_name(self.direction), short, "normal")
        } else {
            self.moved
        };
        let shape = moved.voxel_collision_shape();
        if !shape.is_empty() {
            let along = self.extended_progress(self.progress);
            let [x, y, z] = step(self.direction);
            // The game multiplies as floats, then widens.
            let shift = [
                f64::from(x as f32 * along),
                f64::from(y as f32 * along),
                f64::from(z as f32 * along),
            ];
            shapes.push(Placed {
                cells: Cells::Data(shape),
                shift: Some(shift),
                offset: at,
            });
        }
        shapes
    }
}

/// A piston head facing `facing`, short or not, of the kind `piston_type`.
fn head(facing: &str, short: bool, piston_type: &str) -> BlockState {
    blocks::PISTON_HEAD
        .with_property("facing", facing)
        .and_then(|state| state.with_property("short", if short { "true" } else { "false" }))
        .and_then(|state| state.with_property("type", piston_type))
        .expect("a head has these")
}

/// The boxes of the collision shape of `state` within its block (`toAabbs`, one for
/// each full cell), and their bounds (`bounds`); none where it is empty.
pub(crate) fn boxes_of(state: BlockState) -> Option<(Vec<Aabb>, Aabb)> {
    let shape = state.voxel_collision_shape();
    if shape.is_empty() {
        return None;
    }
    let boxes = Placed {
        cells: Cells::Data(shape),
        shift: None,
        offset: [0.0; 3],
    }
    .boxes();
    let first = *boxes.first()?;
    let bounds = boxes.iter().fold(first, |all, one| all.minmax(*one));
    Some((boxes, bounds))
}

/// `PistonMath.getMovementArea(box, direction, d)`: the slab `d` deep beyond the box's
/// face towards `direction`.
pub(crate) fn movement_area(bounds: Aabb, direction: Face, d: f64) -> Aabb {
    let sign = match direction {
        Face::Down | Face::North | Face::West => -1.0,
        Face::Up | Face::South | Face::East => 1.0,
    };
    let signed = d * sign;
    let low = signed.min(0.0);
    let high = signed.max(0.0);
    let (min, max) = (bounds.min, bounds.max);
    let along = axis(direction);
    let face = if sign > 0.0 { max[along] } else { min[along] };
    let mut a = min;
    let mut b = max;
    a[along] = face + low;
    b[along] = face + high;
    Aabb::new(a, b)
}

/// `PistonMovingBlockEntity.getMovement(area, direction, entity)`: how far the entity's
/// box reaches into the area, from the side it is pushed from.
pub(crate) fn movement(area: Aabb, direction: Face, entity: Aabb) -> f64 {
    match direction {
        Face::East => area.max[0] - entity.min[0],
        Face::West => entity.max[0] - area.min[0],
        Face::Up => area.max[1] - entity.min[1],
        Face::Down => entity.max[1] - area.min[1],
        Face::South => area.max[2] - entity.min[2],
        Face::North => entity.max[2] - area.min[2],
    }
}

/// `Entity.limitPistonMovement`: the move of `vector`, which goes along one axis, cut so
/// that the pistons of this tick move the entity at most [`TICK_MOVEMENT`] along it in
/// all, with `deltas` how far they have moved it already in the tick; nothing where
/// what is left is below the game's 1e-5. Only the first axis with a component counts.
pub(crate) fn limit(vector: Vec3, deltas: &mut [f64; 3]) -> Vec3 {
    if super::length_sqr(vector) <= 1.0e-7 {
        return vector;
    }
    for axis in 0..3 {
        if vector[axis] != 0.0 {
            let total = vector[axis] + deltas[axis];
            // `Mth.clamp`.
            let clamped = if total < -TICK_MOVEMENT {
                -TICK_MOVEMENT
            } else {
                total.min(TICK_MOVEMENT)
            };
            let along = clamped - deltas[axis];
            deltas[axis] = clamped;
            if along.abs() <= f64::from(1.0e-5_f32) {
                return [0.0; 3];
            }
            let mut limited = [0.0; 3];
            limited[axis] = along;
            return limited;
        }
    }
    [0.0; 3]
}

/// Whether a moving piston's block is a slime block, which launches what it pushes.
pub(crate) fn is_slime(state: BlockState) -> bool {
    is_block(state, blocks::SLIME_BLOCK)
}

/// Whether a moving piston's block is a honey block, which carries what stands on it.
pub(crate) fn is_honey(state: BlockState) -> bool {
    is_block(state, blocks::HONEY_BLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pistons_move_an_entity_at_most_0_51_along_an_axis_in_a_tick() {
        let mut deltas = [0.0; 3];
        assert_eq!(limit([0.51, 0.0, 0.0], &mut deltas), [0.51, 0.0, 0.0]);
        // A second push the same tick finds the axis used up.
        assert_eq!(limit([0.3, 0.0, 0.0], &mut deltas), [0.0; 3]);
        // The other way it may go back as far as -0.51 in all.
        assert_eq!(limit([-1.5, 0.0, 0.0], &mut deltas), [-1.02, 0.0, 0.0]);
        // Along y it is untouched by x, and only the first axis counts.
        let mut deltas = [0.0; 3];
        assert_eq!(limit([0.0, 0.2, 0.3], &mut deltas), [0.0, 0.2, 0.0]);
        assert_eq!(deltas, [0.0, 0.2, 0.0]);
    }

    #[test]
    fn the_area_a_block_pushes_lies_beyond_its_face_the_way_it_goes() {
        let block = Aabb::new([2.0, -60.0, 2.0], [3.0, -59.0, 3.0]);
        let east = movement_area(block, Face::East, 0.5);
        assert_eq!(east, Aabb::new([3.0, -60.0, 2.0], [3.5, -59.0, 3.0]));
        let west = movement_area(block, Face::West, 0.5);
        assert_eq!(west, Aabb::new([1.5, -60.0, 2.0], [2.0, -59.0, 3.0]));
        let up = movement_area(block, Face::Up, 0.5);
        assert_eq!(up, Aabb::new([2.0, -59.0, 2.0], [3.0, -58.5, 3.0]));
    }
}
