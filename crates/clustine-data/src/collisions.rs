//! The collision shapes of the block states in the game's own voxel form, which the
//! collider reads (`docs/groundwork/n4-brief.md`, sections 1.4 and 11).
//!
//! The values are in `generated/collisions.bin`, a packed table that `cargo datagen`
//! makes by asking the game itself: for each state the index of its collision shape
//! and of its entity-inside shape, and whether the game counts its collision shape as
//! large; for each distinct shape the boundaries of its cells along each axis and
//! which cells are filled, as the game's `VoxelShape` and its `DiscreteVoxelShape`
//! hold them, and whether it is the one instance `Shapes.block()`, which the game's
//! `BlockCollisions` tests by identity.
//!
//! [`crate::Shape`] has the same shapes as boxes, for what reads boxes (light, the
//! outline a ray hits). The two are numbered apart.

use crate::BlockState;
use crate::packed::{self, Table};

/// The layout of the table this module reads.
pub const LAYOUT: u16 = 1;

const STATES: usize = 0;
const SHAPES: usize = 1;
const COORDS: usize = 2;
const CELLS: usize = 3;

/// The bytes of a row of each section.
pub const ROW_BYTES: [usize; 4] = [5, 12, 8, 1];

/// What a state's row holds for an entity-inside shape that is the entity's own.
const INSIDE_SHAPE_OF_THE_ENTITY: u16 = u16::MAX;

static TABLE: Table<4> = Table::parse(
    include_bytes!("generated/collisions.bin"),
    packed::KIND_COLLISIONS,
    LAYOUT,
    ROW_BYTES,
);

const _: () = assert!(
    TABLE.rows(STATES) == crate::BLOCK_STATE_COUNT as usize,
    "collisions.bin does not have one row for each block state"
);

/// An axis, in the game's order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Axis {
    X = 0,
    Y = 1,
    Z = 2,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];
}

/// A collision shape as the game holds it: cells between boundaries along each axis,
/// each cell filled or not.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct VoxelShape(u16);

/// The entity-inside collision shape of a state (the game's
/// `getEntityInsideCollisionShape`), which decides whether an entity is inside the
/// block for its effects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntityInsideShape {
    /// The same for every entity.
    Shape(VoxelShape),
    /// The entity's own collision shape of the block, or the whole block where that is
    /// empty: powder snow, whose collision shape depends on the entity.
    CollisionOfTheEntity,
}

impl BlockState {
    fn collision_row(self) -> &'static [u8] {
        TABLE.row(STATES, usize::from(self.0))
    }

    /// The game's `getCollisionShape` with the empty context, in its voxel form.
    ///
    /// Panics if the id is not a block state's.
    ///
    /// For bamboo, pointed dripstone and the sulfur spike, whose shapes the game shifts
    /// sideways by their position, this is the shape before the shift, which the
    /// collider adds ([`crate::BlockPhysics::offset_type`]). Powder snow, scaffolding
    /// and liquids answer an entity otherwise than the empty context does
    /// (`docs/groundwork/n4-brief.md`, section 1.4); shulker boxes and moving pistons
    /// answer by their block entity, and the table has their answer without one.
    pub fn voxel_collision_shape(self) -> VoxelShape {
        VoxelShape(packed::row_u16(self.collision_row(), 0))
    }

    /// The game's `hasLargeCollisionShape`: whether the shape may reach out of the
    /// block, so that the collider looks at it from a neighbouring cell too. Fences,
    /// walls, closed fence gates and moving pistons.
    pub fn has_large_collision_shape(self) -> bool {
        self.collision_row()[4] & 1 != 0
    }

    /// The game's `getEntityInsideCollisionShape`.
    pub fn entity_inside_shape(self) -> EntityInsideShape {
        match packed::row_u16(self.collision_row(), 2) {
            INSIDE_SHAPE_OF_THE_ENTITY => EntityInsideShape::CollisionOfTheEntity,
            index => EntityInsideShape::Shape(VoxelShape(index)),
        }
    }
}

/// The number of distinct voxel shapes.
pub fn voxel_shape_count() -> usize {
    TABLE.rows(SHAPES)
}

impl VoxelShape {
    /// The shape of this number, if there is one.
    pub fn by_index(index: u16) -> Option<VoxelShape> {
        (usize::from(index) < voxel_shape_count()).then_some(VoxelShape(index))
    }

    /// The shape's number among the distinct voxel shapes.
    pub fn index(self) -> u16 {
        self.0
    }

    fn head(self) -> &'static [u8] {
        TABLE.row(SHAPES, usize::from(self.0))
    }

    /// Whether this is the instance `Shapes.block()`, the full cube that the game's
    /// `BlockCollisions` yields without testing its cells.
    pub fn is_block(self) -> bool {
        self.head()[0] & 1 != 0
    }

    /// The number of cells along `axis`.
    pub fn size(self, axis: Axis) -> usize {
        usize::from(self.head()[1 + axis as usize])
    }

    /// The boundaries of the cells along `axis`, one more than the cells: the game's
    /// `getCoords(axis)`.
    pub fn coords(self, axis: Axis) -> impl ExactSizeIterator<Item = f64> {
        let mut start = packed::row_u32(self.head(), 4) as usize;
        for before in &Axis::ALL[..axis as usize] {
            start += self.size(*before) + 1;
        }
        (start..start + self.size(axis) + 1).map(|row| packed::row_f64(TABLE.row(COORDS, row), 0))
    }

    /// The boundary `index` along `axis`: the game's `get(axis, index)`. Panics if the
    /// shape has no such boundary.
    pub fn coord(self, axis: Axis, index: usize) -> f64 {
        assert!(index <= self.size(axis), "no such boundary of a shape");
        let mut row = packed::row_u32(self.head(), 4) as usize + index;
        for before in &Axis::ALL[..axis as usize] {
            row += self.size(*before) + 1;
        }
        packed::row_f64(TABLE.row(COORDS, row), 0)
    }

    /// Whether the cell (x, y, z) is filled: the game's `DiscreteVoxelShape.isFull`.
    /// A cell outside the shape is not.
    pub fn is_full(self, x: usize, y: usize, z: usize) -> bool {
        let (xs, ys, zs) = (self.size(Axis::X), self.size(Axis::Y), self.size(Axis::Z));
        if x >= xs || y >= ys || z >= zs {
            return false;
        }
        let bit = (x * ys + y) * zs + z;
        let first = packed::row_u32(self.head(), 8) as usize;
        TABLE.row(CELLS, first + bit / 8)[0] & (1 << (bit % 8)) != 0
    }

    /// Whether no cell is filled: the game's `isEmpty`.
    pub fn is_empty(self) -> bool {
        let count = self.size(Axis::X) * self.size(Axis::Y) * self.size(Axis::Z);
        let first = packed::row_u32(self.head(), 8) as usize;
        (first..first + count.div_ceil(8)).all(|row| TABLE.row(CELLS, row)[0] == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    fn coords(shape: VoxelShape, axis: Axis) -> Vec<f64> {
        shape.coords(axis).collect()
    }

    #[test]
    fn stone_is_the_full_block_instance_and_air_is_empty() {
        let stone = blocks::STONE.voxel_collision_shape();
        assert!(stone.is_block());
        assert!(!stone.is_empty());
        for axis in Axis::ALL {
            assert_eq!(coords(stone, axis), [0.0, 1.0]);
        }
        assert!(stone.is_full(0, 0, 0));
        assert!(!stone.is_full(1, 0, 0));

        let air = blocks::AIR.voxel_collision_shape();
        assert!(air.is_empty() && !air.is_block());
        assert!(!blocks::STONE.has_large_collision_shape());
        assert_eq!(
            blocks::STONE.entity_inside_shape(),
            EntityInsideShape::Shape(stone)
        );
    }

    #[test]
    fn a_bottom_slab_fills_the_lower_of_two_cells() {
        // The game makes a box on the sixteenths as cells of the coarsest resolution
        // that holds it: a slab is two cells high, of which the lower is filled.
        let slab = BlockState::parse("minecraft:oak_slab[type=bottom]")
            .unwrap()
            .voxel_collision_shape();
        assert!(!slab.is_block());
        assert_eq!(coords(slab, Axis::Y), [0.0, 0.5, 1.0]);
        assert_eq!(coords(slab, Axis::X), [0.0, 1.0]);
        assert!(slab.is_full(0, 0, 0) && !slab.is_full(0, 1, 0));
        assert_eq!(slab.coord(Axis::Y, 1), 0.5);
    }

    #[test]
    fn a_fence_is_large_and_reaches_above_its_block() {
        let fence = BlockState::parse("minecraft:oak_fence").unwrap();
        assert!(fence.has_large_collision_shape());
        let shape = fence.voxel_collision_shape();
        assert_eq!(coords(shape, Axis::Y).last(), Some(&1.5));
        assert!(
            BlockState::parse("minecraft:cobblestone_wall")
                .unwrap()
                .has_large_collision_shape()
        );
    }

    #[test]
    fn the_inside_shapes_of_the_five_classes_are_their_own() {
        let inside = |name: &str| BlockState::parse(name).unwrap().entity_inside_shape();
        assert_eq!(
            inside("minecraft:powder_snow"),
            EntityInsideShape::CollisionOfTheEntity
        );
        // A water cauldron's inside is its water, up to its level.
        let EntityInsideShape::Shape(level_one) = inside("minecraft:water_cauldron[level=1]")
        else {
            panic!("a water cauldron's inside does not depend on the entity");
        };
        let EntityInsideShape::Shape(level_three) = inside("minecraft:water_cauldron[level=3]")
        else {
            panic!("a water cauldron's inside does not depend on the entity");
        };
        assert_ne!(level_one, level_three);
        assert!(!level_one.is_block());
        for name in [
            "minecraft:end_portal",
            "minecraft:tripwire",
            "minecraft:lava_cauldron",
        ] {
            let EntityInsideShape::Shape(shape) = inside(name) else {
                panic!("{name}");
            };
            assert!(!shape.is_block() && !shape.is_empty(), "{name}");
        }
        // Every other block's inside is the full block, also where nothing collides.
        let block = blocks::STONE.voxel_collision_shape();
        assert_eq!(inside("minecraft:water"), EntityInsideShape::Shape(block));
        assert_eq!(inside("minecraft:torch"), EntityInsideShape::Shape(block));
    }

    #[test]
    fn shifted_shapes_are_kept_unshifted() {
        // The game's bamboo is a thin post within its cell before the shift its
        // position gives it, which may move it partly out of the cell.
        let bamboo = BlockState::parse("minecraft:bamboo")
            .unwrap()
            .voxel_collision_shape();
        let xs = coords(bamboo, Axis::X);
        assert!(xs.iter().all(|x| (0.0..=1.0).contains(x)), "{xs:?}");
    }

    #[test]
    fn every_shape_of_every_state_is_one_of_the_table() {
        for id in 0..crate::BLOCK_STATE_COUNT {
            let state = BlockState(id as u16);
            let shape = state.voxel_collision_shape();
            assert!(VoxelShape::by_index(shape.index()).is_some());
            // The voxel shapes and the boxes agree on what collides at all.
            assert_eq!(
                shape.is_empty(),
                state.props().collision_is_empty(),
                "state {id}"
            );
        }
    }
}
