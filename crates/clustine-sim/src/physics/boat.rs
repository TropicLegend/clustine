//! What a boat's own tick works out of the blocks about it when no player drives it
//! (`AbstractBoat.getStatus`, `isUnderwater`, `checkInWater`, `getGroundFriction`,
//! `getWaterLevelAbove` and `floatBoat`; `docs/groundwork/n5-brief.md`, sections 4.3
//! and 4.4), as functions of the blocks a [`World`] gives and the boat's body.
//!
//! The arithmetic is the game's: floats where it adds a fluid's height to a block's y
//! and where it keeps its drag, widened where it widens.

use clustine_blocks::data::{has_water, is_class};
use clustine_blocks::fluid::{self, FluidState};
use clustine_data::BlockClass;
use clustine_world::BlockPos;

use super::aabb::Aabb;
use super::body::Body;
use super::collide::{self, Context};
use super::entities::Mover;
use super::{Rules, World, floor};

/// `getDefaultGravity()` of a boat.
pub(crate) const GRAVITY: f64 = 0.04;

/// `getAirDrag()` of a boat, which a bounce reads.
pub(crate) const AIR_DRAG: f32 = 1.0;

/// The ticks under water after which a boat lets its passengers off.
pub(crate) const OUT_OF_CONTROL: f32 = 60.0;

/// `AbstractBoat$Status`, by its ordinals, which the record keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    InWater = 0,
    UnderWater = 1,
    UnderFlowingWater = 2,
    OnLand = 3,
    InAir = 4,
}

/// What the record keeps for a boat that has not ticked yet, whose `status` the game
/// holds as null.
pub(crate) const NO_STATUS: u8 = u8::MAX;

impl Status {
    /// The status of an ordinal of the record, `None` for one not yet worked out.
    pub(crate) fn of(ordinal: u8) -> Option<Self> {
        match ordinal {
            0 => Some(Self::InWater),
            1 => Some(Self::UnderWater),
            2 => Some(Self::UnderFlowingWater),
            3 => Some(Self::OnLand),
            4 => Some(Self::InAir),
            _ => None,
        }
    }

    /// `isUnderWater()`.
    pub(crate) fn is_under_water(status: Option<Self>) -> bool {
        matches!(status, Some(Self::UnderWater | Self::UnderFlowingWater))
    }
}

/// What `getStatus()` found, and the fields it set on its way: `waterLevel` and
/// `landFriction`, where it set them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Found {
    pub(crate) status: Status,
    pub(crate) water_level: f64,
    pub(crate) land_friction: Option<f32>,
}

/// The water of the block at `pos` and its height, where it holds water.
fn water_at<W: World + ?Sized>(world: &W, pos: BlockPos) -> Option<(FluidState, f32)> {
    let state = world.state(pos);
    if !has_water(state) {
        return None;
    }
    let held = FluidState::of(state);
    Some((held, fluid::height(&mut Rules(world), held, pos)))
}

/// `Mth.ceil` of a double.
fn ceil(value: f64) -> i32 {
    value.ceil() as i32
}

/// `isUnderwater()`: water over the top of the box, a source's or a flowing one's,
/// which tells at once.
fn under_water<W: World + ?Sized>(world: &W, bounds: &Aabb) -> Option<Status> {
    let top = bounds.max[1] + 0.001;
    let mut source = false;
    for x in floor(bounds.min[0])..ceil(bounds.max[0]) {
        for y in floor(bounds.max[1])..ceil(top) {
            for z in floor(bounds.min[2])..ceil(bounds.max[2]) {
                let Some((held, height)) = water_at(world, BlockPos::new(x, y, z)) else {
                    continue;
                };
                if top < f64::from(y as f32 + height) {
                    if !held.is_source() {
                        return Some(Status::UnderFlowingWater);
                    }
                    source = true;
                }
            }
        }
    }
    source.then_some(Status::UnderWater)
}

/// `checkInWater()`: whether water reaches above the bottom of the box, and the
/// highest surface met, which stays at the lowest double where none is.
fn check_in_water<W: World + ?Sized>(world: &W, bounds: &Aabb) -> (bool, f64) {
    let mut level = -f64::MAX;
    let mut inside = false;
    for x in floor(bounds.min[0])..ceil(bounds.max[0]) {
        for y in floor(bounds.min[1])..ceil(bounds.min[1] + 0.001) {
            for z in floor(bounds.min[2])..ceil(bounds.max[2]) {
                let Some((_, height)) = water_at(world, BlockPos::new(x, y, z)) else {
                    continue;
                };
                let surface = y as f32 + height;
                level = f64::from(surface).max(level);
                inside |= bounds.min[1] < f64::from(surface);
            }
        }
    }
    (inside, level)
}

/// `CollisionContext.empty()`, which `BlockState.getCollisionShape(level, pos)` uses.
const NO_ENTITY: Context = Context {
    bottom: -f64::MAX,
    fall_distance: 0.0,
    falling_block: false,
    noclip: None,
};

/// `getGroundFriction()`: the mean friction of the blocks whose shapes meet a slab a
/// thousandth thick under the box, a lily pad left out, the corners of the ring
/// around it left out and its edges looked at only beside the box's height. Nothing
/// met makes it a float's not-a-number, which is not above 0: the boat is in the air.
fn ground_friction<W: World + ?Sized>(world: &W, bounds: &Aabb) -> f32 {
    let slab = Aabb::new(
        [bounds.min[0], bounds.min[1] - 0.001, bounds.min[2]],
        [bounds.max[0], bounds.min[1], bounds.max[2]],
    );
    let x0 = floor(slab.min[0]) - 1;
    let x1 = ceil(slab.max[0]) + 1;
    let y0 = floor(slab.min[1]) - 1;
    let y1 = ceil(slab.max[1]) + 1;
    let z0 = floor(slab.min[2]) - 1;
    let z1 = ceil(slab.max[2]) + 1;
    let mut sum = 0.0_f32;
    let mut count = 0_i32;
    for x in x0..x1 {
        for z in z0..z1 {
            let edges = i32::from(x == x0 || x == x1 - 1) + i32::from(z == z0 || z == z1 - 1);
            if edges == 2 {
                continue;
            }
            for y in y0..y1 {
                if edges > 0 && (y == y0 || y == y1 - 1) {
                    continue;
                }
                let pos = BlockPos::new(x, y, z);
                let state = world.state(pos);
                if is_class(state, BlockClass::LilyPadBlock) {
                    continue;
                }
                let Some((shape, _)) = collide::collision_shape(state, pos, NO_ENTITY) else {
                    continue;
                };
                if shape.overlaps_box(&slab) {
                    let friction = state.physics().map_or(0.6, |physics| physics.friction);
                    sum += friction;
                    count += 1;
                }
            }
        }
    }
    sum / count as f32
}

/// `getStatus()` of a boat whose box is `bounds`.
pub(crate) fn status<W: World + ?Sized>(world: &W, bounds: &Aabb) -> Found {
    if let Some(status) = under_water(world, bounds) {
        return Found {
            status,
            water_level: bounds.max[1],
            land_friction: None,
        };
    }
    let (inside, water_level) = check_in_water(world, bounds);
    if inside {
        return Found {
            status: Status::InWater,
            water_level,
            land_friction: None,
        };
    }
    let friction = ground_friction(world, bounds);
    if friction > 0.0 {
        return Found {
            status: Status::OnLand,
            water_level,
            land_friction: Some(friction),
        };
    }
    Found {
        status: Status::InAir,
        water_level,
        land_friction: None,
    }
}

/// `getWaterLevelAbove()`: from the layer of the box's top up, as far as its last fall
/// (`lastYd`) reaches, the first layer whose water does not fill a whole block, at its
/// highest surface; above them all where every layer is full.
pub(crate) fn water_level_above<W: World + ?Sized>(world: &W, bounds: &Aabb, last_yd: f64) -> f32 {
    let x0 = floor(bounds.min[0]);
    let x1 = ceil(bounds.max[0]);
    let y0 = floor(bounds.max[1]);
    let y1 = ceil(bounds.max[1] - last_yd);
    let z0 = floor(bounds.min[2]);
    let z1 = ceil(bounds.max[2]);
    'layers: for y in y0..y1 {
        let mut highest = 0.0_f32;
        for x in x0..x1 {
            for z in z0..z1 {
                if let Some((_, height)) = water_at(world, BlockPos::new(x, y, z)) {
                    highest = highest.max(height);
                }
                if highest >= 1.0 {
                    continue 'layers;
                }
            }
        }
        if highest < 1.0 {
            return y as f32 + highest;
        }
    }
    (y1 + 1) as f32
}

/// What `floatBoat()` reads and writes of a boat beside its body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Floating {
    pub(crate) status: Status,
    pub(crate) old_status: Option<Status>,
    pub(crate) water_level: f64,
    pub(crate) land_friction: f32,
    /// `deltaRotation`.
    pub(crate) turning: f32,
    /// `lastYd`.
    pub(crate) last_yd: f64,
}

/// `floatBoat()` on the side that simulates the boat: a boat come down onto water from
/// the air is set on its surface and does nothing else this tick; otherwise the drag
/// and the gravity of its status, and the water's lift where it floats. No player
/// controls a boat this runs for, so a boat on land never halves its friction.
pub(crate) fn float<W: World + ?Sized>(
    world: &W,
    mover: &Mover,
    body: &mut Body,
    floating: &mut Floating,
) {
    let mut gravity = -GRAVITY;
    let mut buoyancy = 0.0_f64;
    let landing = floating.old_status == Some(Status::InAir)
        && floating.status != Status::InAir
        && floating.status != Status::OnLand;
    if landing {
        // `getY(1.0)`: the top of the box.
        floating.water_level = body.position[1] + f64::from(body.height);
        let bounds = body.bounds();
        let y =
            f64::from(water_level_above(world, &bounds, floating.last_yd) - body.height) + 0.101;
        let lifted = bounds.moved([0.0, y - body.position[1], 0.0]);
        if collide::no_collision(world, body.context(), mover, &lifted) {
            body.position[1] = y;
            body.motion = [
                body.motion[0] * 1.0,
                body.motion[1] * 0.0,
                body.motion[2] * 1.0,
            ];
            floating.last_yd = 0.0;
        }
        floating.status = Status::InWater;
        return;
    }
    // The game starts the drag at 0.05, which every status replaces.
    let drag: f32 = match floating.status {
        Status::InWater => {
            buoyancy = (floating.water_level - body.position[1]) / f64::from(body.height);
            0.9
        }
        Status::UnderFlowingWater => {
            gravity = -7.0e-4;
            0.9
        }
        Status::UnderWater => {
            buoyancy = f64::from(0.01_f32);
            0.45
        }
        Status::InAir => 0.9,
        Status::OnLand => floating.land_friction,
    };
    let [x, y, z] = body.motion;
    body.motion = [x * f64::from(drag), y + gravity, z * f64::from(drag)];
    floating.turning *= drag;
    if buoyancy > 0.0 {
        let [x, y, z] = body.motion;
        body.motion = [x, (y + buoyancy * (GRAVITY / 0.65)) * 0.75, z];
    }
}

/// `noBlocksAround(entity)` of the vehicle check: every block in the box grown by a
/// sixteenth and reaching 0.55 lower is air. **Inferred**: `findBlocksIn` visits the
/// blocks from the floor of each lower bound to the floor of each upper one
/// (`LevelReader` is not in the dump).
pub(crate) fn no_blocks_around<W: World + ?Sized>(world: &W, bounds: &Aabb) -> bool {
    let around = bounds
        .inflate([0.0625; 3])
        .expand_towards([0.0, -0.55, 0.0]);
    for x in floor(around.min[0])..=floor(around.max[0]) {
        for y in floor(around.min[1])..=floor(around.max[1]) {
            for z in floor(around.min[2])..=floor(around.max[2]) {
                if !world.state(BlockPos::new(x, y, z)).props().is_air() {
                    return false;
                }
            }
        }
    }
    true
}
