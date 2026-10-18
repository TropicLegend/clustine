//! How entities other than players move, as the 26.3 jar's classes have it
//! (`docs/groundwork/n4-brief.md`, sections 1 to 5): the game's boxes and voxel
//! shapes, its collider (`Entity.collide`, `Shapes.collide`, `BlockCollisions`), the
//! entity's own move (`Entity.move`), the walk through the blocks a move touches and
//! their effects, the push of water and lava, and the item's tick.
//!
//! The arithmetic is the game's to the bit: `f64` where it uses `double`, `f32` where it
//! uses `float`, widened where Java widens, and no fused multiply-add. Nothing here
//! draws from a random but where the item's tick names it, and nothing reads anything
//! but the blocks a [`World`] gives, so that a move is a function of them.

pub(crate) mod aabb;
pub(crate) mod body;
pub(crate) mod collide;
pub(crate) mod entities;
pub(crate) mod fluids;
pub(crate) mod item;
pub(crate) mod minecart;
pub(crate) mod piston;
pub(crate) mod voxel;
pub(crate) mod walk;

use clustine_data::BlockState;
use clustine_world::{BlockPos, ChunkPos};

/// What a move reads of the world.
pub(crate) trait World {
    /// The block at `pos`, or `None` where its chunk is neither loaded nor copied, or
    /// `pos` is outside the world's height: the game's level reads air there, and gives
    /// no collision for a chunk it does not have (`getChunkForCollisions`).
    fn block(&self, pos: BlockPos) -> Option<BlockState>;

    /// Whether the chunk is loaded or copied (`getChunk(x, z, FULL, false)` is not
    /// null).
    fn has_chunk(&self, chunk: ChunkPos) -> bool;

    /// The block entity of the moving piston at `pos`, where the block there is one
    /// and the world has it: what it collides with depends on how far its block has
    /// gone. A world without block entities has none, and the moving piston is empty.
    fn moving_block(&self, _pos: BlockPos) -> Option<piston::MovingBlock> {
        None
    }

    /// The entities of the region's loaded and copied chunks whose boxes meet `bounds`,
    /// as obstacles, in the order of the game's lookups (`docs/groundwork/n5-brief.md`,
    /// finding 18). A world without entities has none.
    fn entity_colliders(&self, _bounds: &aabb::Aabb) -> Vec<entities::Collider> {
        Vec::new()
    }

    /// The players whose boxes meet `bounds`, as obstacles: players are not entities of
    /// the region's chunks, and none is given yet (a named difference of
    /// `docs/groundwork/n5-plan.md`, section 7 item 3, until the region passes the
    /// boxes it knows).
    fn player_colliders(&self, _bounds: &aabb::Aabb) -> Vec<entities::Collider> {
        Vec::new()
    }

    /// The block at `pos`, air where [`World::block`] has none.
    fn state(&self, pos: BlockPos) -> BlockState {
        self.block(pos).unwrap_or(clustine_data::blocks::AIR)
    }
}

/// The view of a [`World`] that the shared rules of `clustine-blocks` read through,
/// which only reads blocks.
pub(crate) struct Rules<'a, W: World + ?Sized>(pub(crate) &'a W);

impl<W: World + ?Sized> clustine_blocks::BlockView for Rules<'_, W> {
    fn block(&mut self, at: BlockPos) -> BlockState {
        self.0.state(at)
    }

    fn raw_brightness(&mut self, _at: BlockPos) -> Option<u8> {
        None
    }

    fn schedule_block_tick(&mut self, _at: BlockPos, _block: u16, _delay: i32) {}

    fn schedule_fluid_tick(&mut self, _at: BlockPos, _fluid: u8, _delay: i32) {}

    fn rules(&self) -> clustine_blocks::Rules {
        clustine_blocks::Rules::Placing
    }

    fn next_int(&mut self, _bound: u32) -> u32 {
        0
    }
}

/// A vector of the game's `Vec3`.
pub(crate) type Vec3 = [f64; 3];

/// `Vec3.lengthSqr`.
pub(crate) fn length_sqr([x, y, z]: Vec3) -> f64 {
    x * x + y * y + z * z
}

/// `Vec3.horizontalDistanceSqr`.
pub(crate) fn horizontal_sqr([x, _, z]: Vec3) -> f64 {
    x * x + z * z
}

/// `Vec3.add`.
pub(crate) fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `Vec3.subtract`.
pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `Vec3.scale`.
pub(crate) fn scale([x, y, z]: Vec3, by: f64) -> Vec3 {
    [x * by, y * by, z * by]
}

/// `Vec3.distanceToSqr`.
pub(crate) fn distance_sqr(a: Vec3, b: Vec3) -> f64 {
    let (dx, dy, dz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
    dx * dx + dy * dy + dz * dz
}

/// `Vec3.normalize`.
pub(crate) fn normalize(v: Vec3) -> Vec3 {
    clustine_blocks::fluid::normalize(v)
}

/// `Mth.floor` of a double.
pub(crate) fn floor(value: f64) -> i32 {
    value.floor() as i32
}

/// `BlockPos.containing`.
pub(crate) fn containing([x, y, z]: Vec3) -> BlockPos {
    BlockPos::new(floor(x), floor(y), floor(z))
}

/// `Mth.equal` of two doubles: closer than the float 1e-5.
pub(crate) fn nearly_equal(a: f64, b: f64) -> bool {
    (b - a).abs() < f64::from(1.0e-5_f32)
}
