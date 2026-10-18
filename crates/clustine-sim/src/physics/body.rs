//! An entity's body as it moves: its position, motion and box, the flags a move sets,
//! and `Entity.move` for its own moves (`MoverType.SELF`), with the block it stands on,
//! its fall, its bounce and the speed of the block under it
//! (`docs/groundwork/n4-brief.md`, sections 1.2 and 1.5 to 1.8).

use clustine_blocks::data::{FALL_DAMAGE_RESETTING, FENCES, SUPPRESSES_BOUNCE, WALLS, is_class};
use clustine_data::{BlockClass, BlockState, blocks};
use clustine_world::BlockPos;

use super::aabb::{Aabb, lerp};
use super::collide::{self, Context};
use super::{Vec3, World};

/// The game's air drag of entities that do not change it, `0.98f`.
pub(crate) const AIR_DRAG: f32 = 0.98;

/// The gravity of items and falling blocks (`getDefaultGravity`).
pub(crate) const GRAVITY: f64 = 0.04;

/// One move of the tick (`Entity.Movement`): where from and to, and, for a move the
/// entity made itself, the move it asked for, whose axes the walk through the blocks
/// takes one at a time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Movement {
    pub(crate) from: Vec3,
    pub(crate) to: Vec3,
    pub(crate) asked: Option<Vec3>,
}

/// What an entity is to the physics, for one tick: the fields the game keeps on an
/// `Entity` that its movement reads and writes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Body {
    pub(crate) position: Vec3,
    pub(crate) motion: Vec3,
    pub(crate) on_ground: bool,
    pub(crate) fall_distance: f64,
    pub(crate) width: f32,
    pub(crate) height: f32,
    /// Where it was at the start of the tick (`xo`, `yo`, `zo`).
    pub(crate) old_position: Vec3,
    pub(crate) horizontal_collision: bool,
    pub(crate) vertical_collision: bool,
    pub(crate) vertical_collision_below: bool,
    /// `mainSupportingBlockPos`.
    pub(crate) supporting: Option<BlockPos>,
    /// `onGroundNoBlocks`.
    pub(crate) on_ground_no_blocks: bool,
    /// `stuckSpeedMultiplier`: a cobweb's or powder snow's hold, for its next move.
    pub(crate) stuck_speed: Vec3,
    /// `movementThisTick`.
    pub(crate) movements: Vec<Movement>,
    /// `isInWater`, as the last fluid update left it: no fall counts in water.
    pub(crate) in_water: bool,
    /// `syncPosition`: a bounce makes the tick one of the edge's update ticks.
    pub(crate) sync_position: bool,
    /// Whether it is a falling block, which powder snow holds up.
    pub(crate) falling_block: bool,
    /// Whether it is an item, whose friction and speed come from the block a whole
    /// block below it (`ItemEntity.getBlockPosBelowThatAffectsMyMovement`).
    pub(crate) item: bool,
    /// The fall the last move ended and the block it ended on (`Block.fallOn`), where
    /// it ended one.
    pub(crate) fell_on: Option<(f64, BlockPos)>,
}

impl Body {
    /// `getBoundingBox`, made again from the position as `setPos` makes it.
    pub(crate) fn bounds(&self) -> Aabb {
        Aabb::of_entity(self.position, self.width, self.height)
    }

    /// The box of the entity at `at`.
    pub(crate) fn bounds_at(&self, at: Vec3) -> Aabb {
        Aabb::of_entity(at, self.width, self.height)
    }

    /// `CollisionContext.of(entity)`.
    pub(crate) fn context(&self) -> Context {
        Context {
            bottom: self.position[1],
            fall_distance: self.fall_distance,
            falling_block: self.falling_block,
        }
    }

    /// `blockPosition`.
    pub(crate) fn block_position(&self) -> BlockPos {
        super::containing(self.position)
    }

    /// `getOnPos(f)`: the block the entity stands on, as far below its feet as `f`
    /// reaches, or the supporting block itself for a fence, wall or gate.
    pub(crate) fn on_pos<W: World + ?Sized>(&self, world: &W, below: f32) -> BlockPos {
        if let Some(pos) = self.supporting {
            if below > 1.0e-5 {
                let state = world.state(pos);
                if (f64::from(below) <= 0.5 && FENCES.contains(state))
                    || WALLS.contains(state)
                    || is_class(state, BlockClass::FenceGateBlock)
                {
                    return pos;
                }
                return BlockPos::new(
                    pos.x,
                    super::floor(self.position[1] - f64::from(below)),
                    pos.z,
                );
            }
            return pos;
        }
        BlockPos::new(
            super::floor(self.position[0]),
            super::floor(self.position[1] - f64::from(below)),
            super::floor(self.position[2]),
        )
    }

    /// `getOnPosLegacy`.
    pub(crate) fn on_pos_legacy<W: World + ?Sized>(&self, world: &W) -> BlockPos {
        self.on_pos(world, 0.2)
    }

    /// `getBlockPosBelowThatAffectsMyMovement`.
    pub(crate) fn below_for_movement<W: World + ?Sized>(&self, world: &W) -> BlockPos {
        self.on_pos(world, if self.item { 0.999_999 } else { 0.500_001 })
    }

    /// `addMovementThisTick`: at most a hundred, the first two joined when full.
    fn add_movement(&mut self, movement: Movement) {
        if self.movements.len() >= 100 {
            let first = self.movements.remove(0);
            let second = self.movements.remove(0);
            self.movements.insert(
                0,
                Movement {
                    from: first.from,
                    to: second.to,
                    asked: None,
                },
            );
        }
        self.movements.push(movement);
    }

    /// `Entity.move(MoverType.SELF, vector)` for an entity that is not ridden, does not
    /// step up and makes no sound of its movement.
    pub(crate) fn move_self<W: World + ?Sized>(
        &mut self,
        world: &W,
        no_physics: bool,
        vector: Vec3,
    ) {
        if no_physics {
            self.position = super::add(self.position, vector);
            self.horizontal_collision = false;
            self.vertical_collision = false;
            self.vertical_collision_below = false;
            return;
        }
        let mut vector = vector;
        if super::length_sqr(self.stuck_speed) > 1.0e-7 {
            vector = [
                vector[0] * self.stuck_speed[0],
                vector[1] * self.stuck_speed[1],
                vector[2] * self.stuck_speed[2],
            ];
            self.stuck_speed = [0.0; 3];
            self.motion = [0.0; 3];
        }
        let out = collide::collide(world, self.context(), &self.bounds(), vector);
        let moved = super::length_sqr(out);
        if moved > 1.0e-7 || super::length_sqr(vector) - moved < 1.0e-7 {
            if self.fall_distance != 0.0 && moved >= 1.0 {
                let reach = (moved.sqrt()).min(8.0);
                let to = super::add(self.position, super::scale(super::normalize(out), reach));
                if clips_fall_resetting(world, self.position, to) {
                    self.fall_distance = 0.0;
                }
            }
            let from = self.position;
            let to = super::add(from, out);
            self.add_movement(Movement {
                from,
                to,
                asked: Some(vector),
            });
            self.position = to;
        }
        let x_collision = !super::nearly_equal(vector[0], out[0]);
        let z_collision = !super::nearly_equal(vector[2], out[2]);
        self.horizontal_collision = x_collision || z_collision;
        let moved_y = vector[1].abs() > 0.0;
        self.vertical_collision = vector[1] != out[1];
        self.vertical_collision_below = self.vertical_collision && vector[1] < 0.0;
        // `setOnGroundWithMovement`.
        self.on_ground = self.vertical_collision_below;
        self.check_supporting_block(world, Some(out));
        let on = self.on_pos_legacy(world);
        let below = world.state(on);
        self.check_fall_damage(out[1], on);
        if (moved_y && self.vertical_collision) || self.horizontal_collision {
            self.restitute(below, x_collision, z_collision, out);
        }
        let factor = f64::from(self.speed_factor(world));
        self.motion = [
            self.motion[0] * factor,
            self.motion[1] * 1.0,
            self.motion[2] * factor,
        ];
    }

    /// `checkSupportingBlock(onGround, movement)`.
    pub(crate) fn check_supporting_block<W: World + ?Sized>(
        &mut self,
        world: &W,
        movement: Option<Vec3>,
    ) {
        if !self.on_ground {
            self.on_ground_no_blocks = false;
            self.supporting = None;
            return;
        }
        let bounds = self.bounds();
        let probe = Aabb::new(
            [bounds.min[0], bounds.min[1] - 1.0e-6, bounds.min[2]],
            [bounds.max[0], bounds.min[1], bounds.max[2]],
        );
        let context = self.context();
        let mut found = collide::find_supporting_block(world, context, &probe, self.position);
        if found.is_some() || self.on_ground_no_blocks {
            self.supporting = found;
        } else if let Some(movement) = movement {
            found = collide::find_supporting_block(
                world,
                context,
                &probe.moved([-movement[0], 0.0, -movement[2]]),
                self.position,
            );
            self.supporting = found;
        }
        self.on_ground_no_blocks = found.is_none();
    }

    /// `checkFallDamage`: the fall counts while it falls out of water, and ends on the
    /// ground. What it lands on does nothing to an item or a falling block that the
    /// game shows (a honey block's slide and its particles are left out); the fall it
    /// ends is kept for a falling anvil's chipping ([`Body::fell_on`]).
    fn check_fall_damage(&mut self, moved_y: f64, on: BlockPos) {
        self.fell_on = None;
        if !self.in_water && moved_y < 0.0 {
            self.fall_distance -= f64::from(moved_y as f32);
        }
        if self.on_ground {
            if self.fall_distance > 0.0 {
                self.fell_on = Some((self.fall_distance, on));
            }
            self.fall_distance = 0.0;
        }
    }

    /// `restituteMovementAfterCollisions`: what a collision leaves of the motion, a
    /// bouncy block below giving part of a fall back.
    fn restitute(&mut self, below: BlockState, x_collision: bool, z_collision: bool, out: Vec3) {
        let mut bounciness = 0.0_f64;
        let motion = self.motion;
        let mut after = motion;
        if x_collision {
            after[0] = -motion[0] * bounciness;
        }
        if z_collision {
            after[2] = -motion[2] * bounciness;
        }
        let mut bounced = bounciness > 0.0 && (x_collision || z_collision);
        if self.vertical_collision {
            if self.vertical_collision_below {
                bounciness = if -motion[1] <= GRAVITY || SUPPRESSES_BOUNCE.contains(below) {
                    0.0
                } else {
                    bounciness.max(block_bounciness(below))
                };
            }
            let (a, b) = if bounciness > 0.0 {
                let ratio = out[1] / motion[1];
                bounced = true;
                (ratio * GRAVITY, lerp(ratio, 1.0, f64::from(AIR_DRAG)))
            } else {
                (0.0, 1.0)
            };
            after[1] = (a - motion[1]) * b * bounciness;
        }
        if bounced {
            // `bounceOn` does something only on the shelf mushroom, and not to these.
            self.sync_position = true;
        }
        self.motion = after;
    }

    /// `getBlockSpeedFactor`.
    pub(crate) fn speed_factor<W: World + ?Sized>(&self, world: &W) -> f32 {
        let here = world.state(self.block_position());
        let factor = physics_of(here).map_or(1.0, |physics| physics.speed_factor);
        if clustine_blocks::data::is_block(here, blocks::WATER)
            || is_class(here, BlockClass::BubbleColumnBlock)
        {
            return factor;
        }
        if f64::from(factor) != 1.0 {
            return factor;
        }
        let under = world.state(self.below_for_movement(world));
        physics_of(under).map_or(1.0, |physics| physics.speed_factor)
    }

    /// `applyGravity`.
    pub(crate) fn apply_gravity(&mut self) {
        self.motion[1] += -GRAVITY;
    }
}

/// The block's fields of movement, where the state is a block's.
pub(crate) fn physics_of(state: BlockState) -> Option<&'static clustine_data::BlockPhysics> {
    state.physics()
}

/// `getBlockBounciness` for an entity that is not living: four fifths of the block's,
/// as floats.
fn block_bounciness(state: BlockState) -> f64 {
    let restitution = physics_of(state).map_or(0.0, |physics| physics.bounce_restitution);
    f64::from(restitution * 0.8)
}

/// Whether `Level.clip(ClipContext(from, to, FALLDAMAGE_RESETTING, WATER))` hits
/// anything: a block of `#fall_damage_resetting` as a whole block, or water as high as
/// it is.
fn clips_fall_resetting<W: World + ?Sized>(world: &W, from: Vec3, to: Vec3) -> bool {
    super::walk::traverse(from, to, |pos| {
        let state = world.state(pos);
        let mut boxes = Vec::new();
        if FALL_DAMAGE_RESETTING.contains(state) {
            boxes.push(Aabb::of_block(pos));
        }
        if clustine_blocks::data::has_water(state) {
            let fluid = clustine_blocks::fluid::FluidState::of(state);
            let height = clustine_blocks::fluid::height(&mut super::Rules(world), fluid, pos);
            let base = Aabb::of_block(pos);
            boxes.push(Aabb {
                min: base.min,
                max: [base.max[0], base.min[1] + f64::from(height), base.max[2]],
            });
        }
        boxes.iter().any(|shape| clips_shape(shape, from, to))
    })
}

/// `VoxelShape.clip` of a shape of one box: a hit where the segment starts inside it, a
/// thousandth of the way along, or enters it.
pub(crate) fn clips_shape(shape: &Aabb, from: Vec3, to: Vec3) -> bool {
    let delta = super::sub(to, from);
    if super::length_sqr(delta) < 1.0e-7 {
        return false;
    }
    let start = super::add(from, super::scale(delta, 0.001));
    shape.contains(start) || shape.clip(from, to).is_some()
}
