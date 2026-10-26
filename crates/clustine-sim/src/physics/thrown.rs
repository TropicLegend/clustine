//! A thrown item's flight, as the 26.3 jar's `Projectile`, `ThrowableProjectile` and
//! `ProjectileUtil` have it (`docs/groundwork/n7k-brief.md`, section 11.10; the plan's
//! section 10): its shot (`shoot`, `getMovementToShoot`, `shootFromRotation`), its turn
//! towards its motion (`updateRotation`), the margin its hit test grows boxes by
//! (`computeMargin`), and the block it meets along its move (`Level.clip` with
//! `ClipContext.Block.COLLIDER` and `Fluid.NONE`).
//!
//! Its tick, which reads the region's entities and players, is the region's
//! (`region/thrown.rs`); what is here reads nothing but its arguments and a [`World`].

use clustine_world::BlockPos;

use super::aabb::Aabb;
use super::collide::{Context, collision_shape};
use super::{Vec3, World};
use crate::mobs::ai::maths::{atan2, table_cos, table_sin};
use crate::region::Random;

/// `ThrowableProjectile.getDefaultGravity()`.
pub(crate) const GRAVITY: f64 = 0.03;

/// `ThrowableProjectile.getAirDrag()`, and what water keeps of the motion instead
/// (`applyInertia`), as floats.
pub(crate) const AIR_DRAG: f32 = 0.99;
pub(crate) const WATER_DRAG: f32 = 0.8;

/// How far below its thrower's eyes a thrown item starts (`ThrowableItemProjectile`'s
/// constructor of a thrower).
pub(crate) const BELOW_THE_EYES: f64 = 0.100_000_001_490_116_12;

/// What `getMovementToShoot` scales its uncertainty by.
const UNCERTAINTY_SCALE: f64 = 0.017_227_5;

/// The degrees in a radian as `shoot` and `updateRotation` widen them.
const DEGREES: f64 = 57.295_776_367_187_5;

/// `Mth.DEG_TO_RAD`, a float.
const DEGREES_TO_RADIANS: f32 = 0.017_453_292;

/// The snowball's shot by a player (`SnowballItem.use`): its speed and uncertainty.
pub(crate) const PLAYER_POWER: f32 = 1.5;
pub(crate) const PLAYER_UNCERTAINTY: f32 = 1.0;

/// The snow golem's shot (`SnowGolem.performRangedAttack`): its speed, and the
/// uncertainty of `rangedAttackUncertainty`.
pub(crate) const GOLEM_POWER: f32 = 1.6;
pub(crate) const GOLEM_UNCERTAINTY: f32 = 12.0;

/// `RandomSource.triangle(centre, spread)`: two doubles drawn, the first less the
/// second.
pub(crate) fn triangle(random: &mut Random, centre: f64, spread: f64) -> f64 {
    let first = random.next_double();
    let second = random.next_double();
    centre + spread * (first - second)
}

/// `Vec3.normalize()`: of length one, or nothing where it is shorter than `1.0E-5f`.
fn normalize([x, y, z]: Vec3) -> Vec3 {
    let length = (x * x + y * y + z * z).sqrt();
    if length < f64::from(1.0e-5_f32) {
        return [0.0; 3];
    }
    [x / length, y / length, z / length]
}

/// `Projectile.getMovementToShoot(x, y, z, power, uncertainty)`: the direction made of
/// length one, three triangles drawn from the projectile's own random added to it in
/// the order x, y, z, and the whole scaled by the power.
pub(crate) fn movement_to_shoot(
    direction: Vec3,
    power: f32,
    uncertainty: f32,
    random: &mut Random,
) -> Vec3 {
    let spread = UNCERTAINTY_SCALE * f64::from(uncertainty);
    let [x, y, z] = normalize(direction);
    let dx = triangle(random, 0.0, spread);
    let dy = triangle(random, 0.0, spread);
    let dz = triangle(random, 0.0, spread);
    let power = f64::from(power);
    [(x + dx) * power, (y + dy) * power, (z + dz) * power]
}

/// What `shoot` leaves of a projectile: its motion, and its yaw and pitch, which face
/// along the motion (`yRotO` and `xRotO` with them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shot {
    pub(crate) motion: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

/// `Projectile.shoot(x, y, z, power, uncertainty)`.
pub(crate) fn shoot(direction: Vec3, power: f32, uncertainty: f32, random: &mut Random) -> Shot {
    let motion = movement_to_shoot(direction, power, uncertainty, random);
    let [yaw, pitch] = facing(motion);
    Shot { motion, yaw, pitch }
}

/// The yaw and pitch that face along `motion`, as `shoot` sets them: `Mth.atan2` of x
/// over z, and of y over the horizontal length, in degrees, narrowed.
fn facing(motion: Vec3) -> [f32; 2] {
    let horizontal = (motion[0] * motion[0] + motion[2] * motion[2]).sqrt();
    [
        (atan2(motion[0], motion[2]) * DEGREES) as f32,
        (atan2(motion[1], horizontal) * DEGREES) as f32,
    ]
}

/// `Projectile.shootFromRotation(shooter, xRot, yRot, 0, power, uncertainty)` of a
/// shooter looking along `yaw` and `pitch`, moving by `known` (`getKnownMovement`) and
/// on the ground or not: shot along the look by the game's sine table, then the
/// shooter's movement added, but its height where the shooter stands on the ground.
pub(crate) fn shoot_from_rotation(
    yaw: f32,
    pitch: f32,
    power: f32,
    uncertainty: f32,
    known: Vec3,
    on_ground: bool,
    random: &mut Random,
) -> Shot {
    let x = -table_sin(yaw * DEGREES_TO_RADIANS) * table_cos(pitch * DEGREES_TO_RADIANS);
    let y = -table_sin((pitch + 0.0) * DEGREES_TO_RADIANS);
    let z = table_cos(yaw * DEGREES_TO_RADIANS) * table_cos(pitch * DEGREES_TO_RADIANS);
    let shot = shoot(
        [f64::from(x), f64::from(y), f64::from(z)],
        power,
        uncertainty,
        random,
    );
    let lift = if on_ground { 0.0 } else { known[1] };
    Shot {
        motion: [
            shot.motion[0] + known[0],
            shot.motion[1] + lift,
            shot.motion[2] + known[2],
        ],
        ..shot
    }
}

/// `Projectile.lerpRotation(from, to)`: `from` brought within half a turn of `to`, then
/// a fifth of the way to it, in floats.
pub(crate) fn lerp_rotation(from: f32, to: f32) -> f32 {
    let mut from = from;
    while to - from < -180.0 {
        from -= 360.0;
    }
    while to - from >= 180.0 {
        from += 360.0;
    }
    from + 0.2 * (to - from)
}

/// `Projectile.updateRotation()` of a projectile that faced `yaw` and `pitch` as its
/// tick began (`yRotO`, `xRotO`) and moves by `motion` now: the pitch first, then the
/// yaw, each a fifth of the way towards the motion.
pub(crate) fn update_rotation(yaw: f32, pitch: f32, motion: Vec3) -> [f32; 2] {
    let [towards_yaw, towards_pitch] = facing(motion);
    let pitch = lerp_rotation(pitch, towards_pitch);
    let yaw = lerp_rotation(yaw, towards_yaw);
    [yaw, pitch]
}

/// `ProjectileUtil.computeMargin(projectile)` of one whose `tickCount` is `tick_count`:
/// nothing for its first two ticks, then a twentieth more each tick, up to 0.3, in
/// floats.
pub(crate) fn margin(tick_count: u32) -> f32 {
    let grown = (tick_count as f32 - 2.0) / 20.0;
    0.0_f32.max(0.3_f32.min(grown))
}

/// `AABB.getDirection` of one box over a segment that moves by `delta` from `from`,
/// sharing the nearest fraction `nearest` with the boxes before: whether the box is met
/// nearer than any before, `nearest` lowered to where.
fn clip_nearer(bounds: &Aabb, from: Vec3, delta: Vec3, nearest: &mut f64) -> bool {
    let mut hit = false;
    for (axis, other_a, other_b) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
        let side = if delta[axis] > 1.0e-7 {
            bounds.min[axis]
        } else if delta[axis] < -1.0e-7 {
            bounds.max[axis]
        } else {
            continue;
        };
        // `clipPoint`.
        let d = (side - from[axis]) / delta[axis];
        let e = from[other_a] + d * delta[other_a];
        let f = from[other_b] + d * delta[other_b];
        if 0.0 < d
            && d < *nearest
            && bounds.min[other_a] - 1.0e-7 < e
            && e < bounds.max[other_a] + 1.0e-7
            && bounds.min[other_b] - 1.0e-7 < f
            && f < bounds.max[other_b] + 1.0e-7
        {
            *nearest = d;
            hit = true;
        }
    }
    hit
}

/// Where a block stopped a segment: the block, and the point on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlockHit {
    pub(crate) pos: BlockPos,
    pub(crate) location: Vec3,
}

/// `VoxelShape.clip(from, to, pos)` of the block's collision `boxes`: the point a
/// thousandth of the way along where that is inside the shape, else where the segment
/// first enters one of its boxes (`AABB.clip` of the shape's boxes).
fn clip_shape(boxes: &[Aabb], from: Vec3, to: Vec3) -> Option<Vec3> {
    if boxes.is_empty() {
        return None;
    }
    let delta = super::sub(to, from);
    if super::length_sqr(delta) < 1.0e-7 {
        return None;
    }
    let inside = super::add(from, super::scale(delta, 0.001));
    if boxes.iter().any(|bounds| bounds.contains(inside)) {
        return Some(inside);
    }
    let mut nearest = 1.0;
    let mut hit = false;
    for bounds in boxes {
        hit |= clip_nearer(bounds, from, delta, &mut nearest);
    }
    hit.then(|| {
        [
            from[0] + nearest * delta[0],
            from[1] + nearest * delta[1],
            from[2] + nearest * delta[2],
        ]
    })
}

/// `Level.clip(ClipContext(from, to, COLLIDER, NONE, projectile))`: the first block on
/// the way, in the order `traverseBlocks` walks them, whose collision shape, as
/// `context` sees it, the segment meets; `None` for a miss, whose location is `to`. The
/// world border is left out: Clustine has none.
pub(crate) fn clip_blocks<W: World + ?Sized>(
    world: &W,
    context: Context,
    from: Vec3,
    to: Vec3,
) -> Option<BlockHit> {
    let mut found = None;
    super::walk::traverse(from, to, |pos| {
        let state = world.state(pos);
        let boxes: Vec<Aabb> = collision_shape(state, pos, context)
            .map(|(shape, _)| shape.boxes())
            .unwrap_or_default();
        match clip_shape(&boxes, from, to) {
            Some(location) => {
                found = Some(BlockHit { pos, location });
                true
            }
            None => false,
        }
    });
    found
}

/// Where a snow golem's snowball starts and which way it is shot
/// (`SnowGolem.performRangedAttack(target, power)` and the shot it hands
/// `Projectile.spawnProjectile`): from the golem's x and z and a tenth below its eyes,
/// at the target's feet across and a fifth of the horizontal distance above 1.1 below
/// its eyes, from the snowball's height. `golem` and `target` are the feet, and
/// `golem_eyes` and `target_eyes` the heights of the eyes above the world's 0.
pub(crate) fn golem_aim(golem: Vec3, golem_eyes: f64, target: Vec3, target_eyes: f64) -> [Vec3; 2] {
    let dx = target[0] - golem[0];
    let dy = target_eyes - 1.100_000_023_841_858;
    let dz = target[2] - golem[2];
    let lift = (dx * dx + dz * dz).sqrt() * 0.200_000_002_980_232_24;
    let at = [golem[0], golem_eyes - BELOW_THE_EYES, golem[2]];
    [at, [dx, dy + lift - at[1], dz]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_margin_is_nothing_for_two_ticks_and_grows_by_a_twentieth_to_three_tenths() {
        assert_eq!(margin(1), 0.0);
        assert_eq!(margin(2), 0.0);
        assert_eq!(margin(3), 0.05);
        assert_eq!(margin(4), 0.1);
        assert_eq!(margin(8), 0.3);
        assert_eq!(margin(100), 0.3);
    }

    #[test]
    fn a_rotation_goes_a_fifth_of_the_way_the_short_way_round() {
        assert_eq!(lerp_rotation(0.0, 10.0), 2.0);
        // From 170 to -170 is twenty degrees on through 180, not 340 back.
        assert_eq!(lerp_rotation(170.0, -170.0), -186.0);
        assert_eq!(lerp_rotation(-170.0, 170.0), 186.0);
    }

    #[test]
    fn a_segment_inside_a_block_is_stopped_a_thousandth_of_the_way_along() {
        let block = [Aabb::of_block(BlockPos::new(0, 0, 0))];
        assert_eq!(
            clip_shape(&block, [0.5, 0.5, 0.5], [1.5, 0.5, 0.5]),
            Some([0.501, 0.5, 0.5])
        );
        assert_eq!(
            clip_shape(&block, [-1.0, 0.5, 0.5], [0.5, 0.5, 0.5]),
            Some([0.0, 0.5, 0.5])
        );
        assert_eq!(clip_shape(&block, [-1.0, 1.5, 0.5], [0.5, 1.5, 0.5]), None);
    }

    /// `SnowGolem.performRangedAttack` by hand: a target 3 east and 4 south, its eyes
    /// 1.3 up, of a golem whose eyes are 1.7 up, both on a floor at 64.
    #[test]
    fn a_snow_golem_aims_above_its_target_by_a_fifth_of_the_distance() {
        let [at, direction] = golem_aim([0.5, 64.0, 0.5], 65.7, [3.5, 64.0, 4.5], 65.3);
        assert_eq!(at, [0.5, 65.7 - 0.100_000_001_490_116_12, 0.5]);
        let lift = 5.0 * 0.200_000_002_980_232_24;
        let dy = 65.3 - 1.100_000_023_841_858;
        assert_eq!(direction, [3.0, dy + lift - at[1], 4.0]);
        // About 0.4 below the snowball: up a fifth of five, from 1.1 below the eyes.
        assert!((direction[1] + 0.4).abs() < 1.0e-6, "{direction:?}");
        // Its shot draws six doubles, as any.
        let mut random = Random::of_entity(1, clustine_world::EntityId(1), 0);
        let start = random.clone();
        let shot = shoot(direction, GOLEM_POWER, GOLEM_UNCERTAINTY, &mut random);
        assert_eq!(random.draws_since(&start), 6);
        let speed = shot.motion.iter().map(|v| v * v).sum::<f64>().sqrt();
        assert!((speed - 1.6).abs() < 0.2, "{shot:?}");
    }

    #[test]
    fn a_shot_with_no_uncertainty_draws_six_doubles_and_goes_where_it_is_aimed() {
        let mut random = Random::of_entity(1, clustine_world::EntityId(1), 0);
        let start = random.clone();
        let shot = shoot([0.0, 0.0, 2.0], 1.5, 0.0, &mut random);
        assert_eq!(random.draws_since(&start), 6);
        assert_eq!(shot.motion, [0.0, 0.0, 1.5]);
        assert_eq!(shot.yaw, 0.0);
        assert_eq!(shot.pitch, 0.0);
    }
}
