//! `LivingEntity.travel` of a body with nothing to steer it (its input is zero): in
//! the air with its block's friction, the drag of the air and gravity; in water slowed
//! and sinking slowly; in lava halved; each move the body's own (`Entity.move`, with a
//! living body's bounce and the fluids looked at again before its fall counts).

use clustine_blocks::data::{in_tag, is_block, is_class};
use clustine_data::{BlockClass, blocks};

use super::{Living, modified_friction, refresh_fluids};
use crate::physics::aabb::Aabb;
use crate::physics::body::{Body, Bounce, physics_of};
use crate::physics::collide;
use crate::physics::item::{Region as Holds, hold_back};
use crate::physics::{self, Vec3, World};

/// How far a body on a ladder may move sideways and down a tick (`0.15f`, widened).
const CLIMBING_SPEED: f64 = 0.150_000_005_960_464_48;

/// `0.8f` widened, the vertical slowing of water and shallow lava.
const FLUID_VERTICAL_DRAG: f64 = 0.800_000_011_920_929;

impl Living {
    /// `travel(Vec3.ZERO)`: in a fluid where it is in one (`shouldTravelInFluid`: a
    /// living body is moved by fluids and stands on none), else in the air.
    pub(super) fn travel<R: Holds + ?Sized>(&mut self, world: &R) {
        if self.body.in_water || self.in_lava() {
            self.travel_in_fluid(world);
        } else {
            self.travel_in_air(world);
        }
    }

    /// `travelInAir(Vec3.ZERO)`.
    fn travel_in_air<R: Holds + ?Sized>(&mut self, world: &R) {
        let below = self.body.below_for_movement(world);
        let friction = if self.body.on_ground {
            let block = physics_of(world.state(below)).map_or(0.6, |physics| physics.friction);
            modified_friction(block, self.attributes.friction_modifier as f32)
        } else {
            1.0
        };
        let moved = self.handle_relative_friction_and_calculate_movement(world, friction);
        // The server always has the chunk it ticks in, and nothing levitates.
        let y = moved[1] - self.attributes.gravity;
        let modifier = self.attributes.air_drag_modifier as f32;
        let horizontal = modified_friction(0.91, modifier);
        let sideways = friction * horizontal;
        let vertical = modified_friction(0.98, modifier);
        self.body.motion = [
            moved[0] * f64::from(sideways),
            y * f64::from(vertical),
            moved[2] * f64::from(sideways),
        ];
    }

    /// `handleRelativeFrictionAndCalculateMovement(Vec3.ZERO, friction)`: a ladder's
    /// hold, the move, and a climb where it runs into a ladder or walks on powder snow.
    fn handle_relative_friction_and_calculate_movement<R: Holds + ?Sized>(
        &mut self,
        world: &R,
        friction: f32,
    ) -> Vec3 {
        let _ = friction;
        self.move_relative_by_nothing();
        self.body.motion = self.handle_on_climbable(world, self.body.motion);
        let motion = self.body.motion;
        self.move_self(world, motion);
        let mut moved = self.body.motion;
        if self.body.horizontal_collision
            && (self.on_climbable(world) || self.was_in_powder_snow && self.walks_on_powder_snow)
        {
            moved[1] = 0.2;
        }
        moved
    }

    /// `moveRelative(speed, Vec3.ZERO)`: nothing to steer by adds a zero vector, which
    /// still makes a negative zero positive.
    fn move_relative_by_nothing(&mut self) {
        self.body.motion = physics::add(self.body.motion, [0.0; 3]);
    }

    /// `onClimbable()`: the block it is in is climbable, or an open trapdoor over a
    /// ladder that faces as it does.
    pub(super) fn on_climbable<W: World + ?Sized>(&self, world: &W) -> bool {
        let at = self.body.block_position();
        let state = world.state(at);
        if in_tag(state, "minecraft:climbable") {
            return true;
        }
        if is_class(state, BlockClass::TrapDoorBlock) && clustine_blocks::data::flag(state, "open")
        {
            let below = world.state(at.offset(0, -1, 0));
            return is_block(below, blocks::LADDER)
                && below.property("facing") == state.property("facing");
        }
        false
    }

    /// `handleOnClimbable(motion)`: on a ladder the fall ends and the move is held to a
    /// climber's speed (a player alone can hold still on one).
    fn handle_on_climbable<W: World + ?Sized>(&mut self, world: &W, motion: Vec3) -> Vec3 {
        if !self.on_climbable(world) {
            return motion;
        }
        self.body.fall_distance = 0.0;
        [
            motion[0].clamp(-CLIMBING_SPEED, CLIMBING_SPEED),
            motion[1].max(-CLIMBING_SPEED),
            motion[2].clamp(-CLIMBING_SPEED, CLIMBING_SPEED),
        ]
    }

    /// `travelInFluid(Vec3.ZERO)`.
    fn travel_in_fluid<R: Holds + ?Sized>(&mut self, world: &R) {
        let falling = self.body.motion[1] <= 0.0;
        let start_y = self.body.position[1];
        let gravity = self.attributes.gravity;
        if self.body.in_water {
            self.travel_in_water(world, gravity, falling, start_y);
        } else {
            self.travel_in_lava(world, gravity, falling, start_y);
        }
    }

    /// `travelInWater(Vec3.ZERO, gravity, falling, startY)`.
    fn travel_in_water<R: Holds + ?Sized>(
        &mut self,
        world: &R,
        gravity: f64,
        falling: bool,
        start_y: f64,
    ) {
        // `getWaterSlowDown`; a body that does not sprint, without dolphin's grace.
        let mut slow = 0.8_f32;
        let mut efficiency = self.attributes.water_movement_efficiency as f32;
        if !self.body.on_ground {
            efficiency *= 0.5;
        }
        if efficiency > 0.0 {
            slow += (0.546_000_06 - slow) * efficiency;
        }
        self.move_relative_by_nothing();
        let motion = self.body.motion;
        self.move_self(world, motion);
        let mut moved = self.body.motion;
        if self.body.horizontal_collision && self.on_climbable(world) {
            moved[1] = 0.2;
        }
        let moved = [
            moved[0] * f64::from(slow),
            moved[1] * FLUID_VERTICAL_DRAG,
            moved[2] * f64::from(slow),
        ];
        self.body.motion = fluid_falling_adjusted(gravity, falling, moved);
        self.jump_out_of_fluid(world, start_y);
    }

    /// `travelInLava(Vec3.ZERO, gravity, falling, startY)`.
    fn travel_in_lava<R: Holds + ?Sized>(
        &mut self,
        world: &R,
        gravity: f64,
        falling: bool,
        start_y: f64,
    ) {
        self.move_relative_by_nothing();
        let motion = self.body.motion;
        self.move_self(world, motion);
        // `isInShallowFluid(#lava)`: no deeper than the fluid jump threshold.
        let threshold = if self.eye_height < 0.4 { 0.0 } else { 0.4 };
        if self.fluids.lava_height <= threshold {
            let [x, y, z] = self.body.motion;
            let slowed = [x * 0.5, y * FLUID_VERTICAL_DRAG, z * 0.5];
            self.body.motion = fluid_falling_adjusted(gravity, falling, slowed);
        } else {
            self.body.motion = physics::scale(self.body.motion, 0.5);
        }
        if gravity != 0.0 {
            self.body.motion = physics::add(self.body.motion, [0.0, -gravity / 4.0, 0.0]);
        }
        self.jump_out_of_fluid(world, start_y);
    }

    /// `jumpOutOfFluid(startY)`: a body against a wall that a little lift would free
    /// is lifted out.
    fn jump_out_of_fluid<W: World + ?Sized>(&mut self, world: &W, start_y: f64) {
        let [x, y, z] = self.body.motion;
        let lift = y + 0.600_000_023_841_857_9 - self.body.position[1] + start_y;
        if self.body.horizontal_collision && self.is_free(world, [x, lift, z]) {
            self.body.motion = [x, 0.300_000_011_920_928_96, z];
        }
    }

    /// `isFree(dx, dy, dz)`: its box moved so would meet nothing and hold no fluid.
    fn is_free<W: World + ?Sized>(&self, world: &W, by: Vec3) -> bool {
        let moved = self.body.bounds().moved(by);
        collide::no_collision(world, self.body.context(), &self.body.mover, &moved)
            && !contains_any_liquid(world, &moved)
    }

    /// `move(MoverType.SELF, motion)` of a living body, and the region's guard after it.
    pub(super) fn move_self<R: Holds + ?Sized>(&mut self, world: &R, motion: Vec3) {
        let bounce = Bounce {
            gravity: self.attributes.gravity,
            air_drag: self.air_drag(),
            own: self.attributes.bounciness,
            living: true,
        };
        let before = self.body.position;
        let mut refreshed = None;
        // `LivingEntity.checkFallDamage`: out of water the fluids are looked at again
        // before the fall counts.
        let mut before_fall = |body: &mut Body| {
            if !body.in_water {
                refreshed = Some(refresh_fluids(body, world));
            }
        };
        self.body.move_with(
            world,
            self.no_physics,
            motion,
            None,
            bounce,
            &mut before_fall,
        );
        if let Some(found) = refreshed {
            self.fluids = found;
        }
        if let Some((distance, on)) = self.body.fell_on.take() {
            self.landed(world, distance, on);
        }
        hold_back(&mut self.body, world, before);
    }
}

/// `getFluidFallingAdjustedMovement(gravity, falling, motion)` of a body that does not
/// sprint: it sinks by a sixteenth of its gravity, held at a slow sink near rest.
fn fluid_falling_adjusted(gravity: f64, falling: bool, motion: Vec3) -> Vec3 {
    if gravity == 0.0 {
        return motion;
    }
    let y = if falling
        && (motion[1] - 0.005).abs() >= 0.003
        && (motion[1] - gravity / 16.0).abs() < 0.003
    {
        -0.003
    } else {
        motion[1] - gravity / 16.0
    };
    [motion[0], y, motion[2]]
}

/// `containsAnyLiquid(box)`: a fluid in any block the box reaches into.
fn contains_any_liquid<W: World + ?Sized>(world: &W, bounds: &Aabb) -> bool {
    let x0 = physics::floor(bounds.min[0]);
    let y0 = physics::floor(bounds.min[1]);
    let z0 = physics::floor(bounds.min[2]);
    let x1 = bounds.max[0].ceil() as i32 - 1;
    let y1 = bounds.max[1].ceil() as i32 - 1;
    let z1 = bounds.max[2].ceil() as i32 - 1;
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                let state = world.state(clustine_world::BlockPos::new(x, y, z));
                if clustine_blocks::fluid::FluidState::of(state)
                    .kind()
                    .is_some()
                {
                    return true;
                }
            }
        }
    }
    false
}
