//! The check of a move a driver's client made of the vehicle it drives
//! (`ServerGamePacketListenerImpl.handleMoveVehicle` and the vehicle's part of
//! `tickPlayer`; `docs/groundwork/n5-brief.md`, findings 12 and 13 and sections 2.2 and
//! 2.3), as functions of the connection's state ([`VehicleCheck`]), the root as it is
//! and the move: what the move does to the root, what the client is to be sent back, and
//! the state after. The region applies a player's moves through these one at a time as
//! they come, and the riders' step applies those kept across a hand-over the same way
//! (`docs/groundwork/n5-plan.md`, section 7 item 4 and section 8 item 12).
//!
//! An accepted move puts the root where the client says, not where the server's own
//! move of it would have ended: only the horizontal error of that move is judged, and
//! a vehicle stuck in something before the move is let through even when its move is
//! wrong. A refused move leaves the root where it was, turned as the client turned it.

use crate::api::VehicleMove;
use crate::physics::aabb::Aabb;
use crate::physics::body::{Body, Bounce};
use crate::physics::collide::{self, Context};
use crate::physics::voxel::Placed;
use crate::physics::{self, Vec3, World};
use crate::state::VehicleCheck;
use clustine_world::Double;

/// The furthest from the world's middle a vehicle is put along x and z, and along y.
const MAX_HORIZONTAL: f64 = 3.0e7;
const MAX_VERTICAL: f64 = 2.0e7;

/// `Entity.absSnapTo`'s and `absSnapRotationTo`'s rotations: the yaw within a turn, the
/// pitch within a quarter turn either way.
pub(crate) fn snapped_rotation(yaw: f32, pitch: f32) -> (f32, f32) {
    (yaw % 360.0, pitch.clamp(-90.0, 90.0) % 360.0)
}

/// `Mth.wrapDegrees(float)`.
fn wrap_degrees(degrees: f32) -> f32 {
    let mut wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// `containsInvalidValues`: a coordinate that is not a number, or a rotation that is
/// not finite, which disconnects the player.
pub(crate) fn invalid(moved: &VehicleMove) -> bool {
    let at = moved.position;
    at.x.is_nan()
        || at.y.is_nan()
        || at.z.is_nan()
        || !moved.yaw.is_finite()
        || !moved.pitch.is_finite()
}

/// The root of the driver's unit as the check reads and moves it.
#[derive(Debug, Clone)]
pub(crate) struct Root {
    /// Its body, with what its last move left of it (`verticalCollisionBelow`, the
    /// movements recorded since its last tick).
    pub(crate) body: Body,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    /// `getGravity()`.
    pub(crate) gravity: f64,
    /// What its move's bounce reads of it.
    pub(crate) bounce: Bounce,
    /// Whether it rides something itself, which its fall does not count then.
    pub(crate) passenger: bool,
    /// `lastYd`, which a boat's fall check sets (`AbstractBoat.checkFallDamage`).
    pub(crate) last_yd: f64,
    /// Whether it is a living entity (a ridden pig), which moves as a living body steps
    /// and falls as `Entity.checkFallDamage` has it, where a boat moves and falls as a
    /// boat (`docs/groundwork/n7h-riding-plan.md`, section 10 item 3).
    pub(crate) living: bool,
    /// `isFlyingVehicle()` (an adult happy ghast): its driver is never kicked for
    /// floating, and its move gathers no fall (its `checkFallDamage` does nothing).
    pub(crate) flying: bool,
}

/// What became of a move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Passed over: the player's last tick did not find them driving the root, which
    /// only a move sent before that tick does (the game's `root != lastVehicle`).
    Ignored,
    /// Taken: the root is where the client put it.
    Accepted,
    /// Further from where the root was than its motion allows: the root is untouched.
    TooQuickly,
    /// Moved wrongly, or into something it was not in before: the root is back where
    /// it was, turned as the client turned it.
    Wrongly,
}

/// The outcome of a move: the verdict, the root after it, the state after it, and,
/// for a refused move, where the client is to be told the root is
/// (`ClientboundMoveVehiclePacket.fromEntity`): its position, yaw and pitch.
#[derive(Debug, Clone)]
pub(crate) struct Checked {
    pub(crate) verdict: Verdict,
    pub(crate) root: Root,
    pub(crate) check: VehicleCheck,
    pub(crate) correction: Option<(Vec3, f32, f32)>,
}

fn doubles(values: [Double; 3]) -> Vec3 {
    values.map(|value| value.0)
}

fn kept(values: Vec3) -> [Double; 3] {
    values.map(Double)
}

/// `getMaximumFlyingTicks(entity)`: how long a vehicle of that gravity may be held in
/// the air (160 ticks for a boat or a cart), without end for one nearly weightless.
pub(crate) fn maximum_flying_ticks(gravity: f64) -> u32 {
    if gravity < f64::from(1.0e-5_f32) {
        return u32::MAX;
    }
    (80.0 * (0.08 / gravity).max(1.0)).ceil() as u32
}

/// The boat's fall check (`AbstractBoat.checkFallDamage`) as the body's own check of
/// its fall reads it: the fall is counted only over what is not water below the block
/// it is in, and `lastYd` is the motion's height as the move ends. A root that rides
/// something counts no fall at all.
fn boat_fall<'a, W: World + ?Sized>(
    world: &'a W,
    passenger: bool,
    last_yd: &'a mut f64,
) -> impl FnMut(&mut Body) + 'a {
    move |body: &mut Body| {
        *last_yd = body.motion[1];
        let below = body.block_position();
        let below = clustine_world::BlockPos::new(below.x, below.y - 1, below.z);
        body.in_water = passenger || clustine_blocks::data::has_water(world.state(below));
    }
}

/// `Entity.move(type, vector)` of a boat, with its own fall check; the fall a ridden
/// root counts is put back.
pub(crate) fn move_boat<W: World + ?Sized>(world: &W, root: &mut Root, vector: Vec3) {
    let fall = root.body.fall_distance;
    let in_water = root.body.in_water;
    let mut last_yd = root.last_yd;
    {
        let mut check = boat_fall(world, root.passenger, &mut last_yd);
        root.body
            .move_with(world, false, vector, None, root.bounce, &mut check);
    }
    root.last_yd = last_yd;
    if root.passenger {
        root.body.fall_distance = fall;
    }
    root.body.in_water = in_water;
}

/// `isEntityCollidingWithAnythingNew(level, root, oldBox, x, y, z)`: whether the root's
/// box, moved to where the client says, meets an entity or a block shape that its box
/// before the move did not already overlap.
fn colliding_anew<W: World + ?Sized>(world: &W, root: &Root, old: &Aabb, at: Vec3) -> bool {
    let body = &root.body;
    let bounds = body.bounds();
    let by = physics::sub(at, body.position);
    let new = bounds.moved(by).deflate(9.999_999_747_378_752e-6);
    let before = old.deflate(9.999_999_747_378_752e-6);
    // `getPreMoveCollisions`: the entities' boxes, then the blocks' shapes judged by
    // the old feet.
    let mut shapes: Vec<Placed> = collide::entity_collisions(world, &body.mover, &new)
        .iter()
        .map(collide::box_shape)
        .collect();
    let context = Context {
        bottom: old.min[1],
        ..body.context()
    };
    shapes.extend(
        collide::block_collisions(world, context, &new)
            .into_iter()
            .map(|found| found.shape),
    );
    shapes.iter().any(|shape| !shape.overlaps_box(&before))
}

/// The controlling branch of `handleMoveVehicle` for a move of the root by the player
/// who drives it, and whose last tick found them driving it (`check.good` is set): the
/// test of moving too quickly, the root's own move, the test of moving wrongly, then
/// the root put where the client says, or back. `allow_flight` is the server's, which
/// spares a vehicle held in the air. Without `test_speed` the test of moving too
/// quickly is not made: for the first of the moves kept across a hand-over after some
/// were dropped, as the game never corrects a driver's first move
/// (`docs/groundwork/n5-plan.md`, section 8 item 10).
pub(crate) fn check_move<W: World + ?Sized>(
    world: &W,
    check: &VehicleCheck,
    mut root: Root,
    moved: &VehicleMove,
    allow_flight: bool,
    test_speed: bool,
) -> Checked {
    let mut check = *check;
    let Some([first_good, last_good]) = check.good else {
        return Checked {
            verdict: Verdict::Ignored,
            correction: None,
            root,
            check,
        };
    };
    let start = root.body.position;
    let x = moved.position.x.clamp(-MAX_HORIZONTAL, MAX_HORIZONTAL);
    let y = moved.position.y.clamp(-MAX_VERTICAL, MAX_VERTICAL);
    let z = moved.position.z.clamp(-MAX_HORIZONTAL, MAX_HORIZONTAL);
    let yaw = wrap_degrees(moved.yaw);
    let pitch = wrap_degrees(moved.pitch);
    let at = [x, y, z];
    // Moved too quickly: further from where it was than ten blocks beyond its motion.
    let [dx, dy, dz] = physics::sub(at, doubles(first_good));
    let motion = physics::length_sqr(root.body.motion);
    let distance = dx * dx + dy * dy + dz * dz;
    if test_speed && distance - motion > 100.0 {
        return Checked {
            verdict: Verdict::TooQuickly,
            correction: Some((start, root.yaw, root.pitch)),
            root,
            check,
        };
    }
    let old = root.body.bounds();
    let vector = physics::sub(at, doubles(last_good));
    let was_below = root.body.vertical_collision_below;
    let recorded = root.body.movements.len();
    if root.living {
        // `Entity.move` checks the fall only where its instance is authoritative, which
        // the server is not of a mob a player's client drives: its fall is the client's
        // ground's alone, below.
        let fall = root.body.fall_distance;
        root.body
            .move_with(world, false, vector, None, root.bounce, &mut |_| {});
        root.body.fall_distance = fall;
        root.body.fell_on = None;
    } else {
        move_boat(world, &mut root, vector);
    }
    let requested_dy = vector[1];
    // Moved wrongly: only the error along x and z counts (the game zeroes every finite
    // error along y).
    let [ex, _, ez] = physics::sub(at, root.body.position);
    let ey = 0.0;
    let error = ex * ex + ey * ey + ez * ez;
    let wrongly = error > 0.0625;
    let free_before = collide::no_collision(world, root.body.context(), &root.body.mover, &old);
    if (wrongly && free_before) || colliding_anew(world, &root, &old, at) {
        let (yaw, pitch) = snapped_rotation(yaw, pitch);
        root.body.position = start;
        root.body.old_position = start;
        root.yaw = yaw;
        root.pitch = pitch;
        // `removeLatestMovementRecording`: the check's own move is not walked, nor a
        // fall it ended.
        root.body.movements.truncate(recorded);
        root.body.fell_on = None;
        return Checked {
            verdict: Verdict::Wrongly,
            correction: Some((start, yaw, pitch)),
            root,
            check,
        };
    }
    let (yaw, pitch) = snapped_rotation(yaw, pitch);
    root.body.position = at;
    root.body.old_position = at;
    root.yaw = yaw;
    root.pitch = pitch;
    let moved_by = physics::sub(at, start);
    // `setOnGroundWithMovement(onGround, moved)` and `doCheckFallDamage`: the client's
    // ground, and the fall over what it says it moved.
    root.body.on_ground = moved.on_ground;
    root.body.check_supporting_block(world, Some(moved_by));
    root.last_yd = root.body.motion[1];
    if !root.passenger && root.living {
        // `Entity.checkFallDamage` with the client's ground, which ends the fall that
        // `LivingEntity.causeFallDamage` then hurts it by (the region, from `fell_on`).
        // The drop first, where it is not in water, then the landing, as the game's
        // order has it.
        let below = root.body.on_pos_legacy(world);
        let in_water = clustine_blocks::data::has_water(world.state(root.body.block_position()));
        if !in_water && moved_by[1] < 0.0 {
            root.body.fall_distance -= f64::from(moved_by[1] as f32);
        }
        if moved.on_ground {
            if root.body.fall_distance > 0.0 {
                root.body.fell_on = Some((root.body.fall_distance, below));
            }
            root.body.fall_distance = 0.0;
        }
    } else if !root.passenger && !root.flying {
        if moved.on_ground {
            root.body.fall_distance = 0.0;
        } else {
            let below = root.body.block_position();
            let below = clustine_world::BlockPos::new(below.x, below.y - 1, below.z);
            if !clustine_blocks::data::has_water(world.state(below)) && moved_by[1] < 0.0 {
                root.body.fall_distance -= f64::from(moved_by[1] as f32);
            }
        }
    }
    check.floating = requested_dy >= -0.03125
        && !was_below
        && !allow_flight
        && !root.flying
        && root.gravity > 0.0
        && super::boat::no_blocks_around(world, &root.body.bounds());
    check.good = Some([first_good, kept(at)]);
    Checked {
        verdict: Verdict::Accepted,
        root,
        check,
        correction: None,
    }
}

/// The state with the checks of the next moves counting from `position`, the root's,
/// as the end of a driver's tick leaves it (`vehicleFirstGood = vehicleLastGood`): what
/// a region seats a driver who comes across a line with before it applies their kept
/// moves (`docs/groundwork/n5-plan.md`, section 7 item 4).
pub(crate) fn seeded(check: &VehicleCheck, position: Vec3) -> VehicleCheck {
    VehicleCheck {
        good: Some([kept(position), kept(position)]),
        ..*check
    }
}

/// The branch of `handleMoveVehicle` for a move sent by a player who rides a root they
/// do not drive: they are told where it is when they were last told more than twenty
/// ticks ago, or the move is more than a block from it; the state's age starts again.
pub(crate) fn passenger_move(
    check: &VehicleCheck,
    root_position: Vec3,
    root_yaw: f32,
    root_pitch: f32,
    moved: &VehicleMove,
) -> (VehicleCheck, Option<(Vec3, f32, f32)>) {
    let mut check = *check;
    let at = [moved.position.x, moved.position.y, moved.position.z];
    if check.since_reset >= VehicleCheck::LONG_AGO || physics::distance_sqr(root_position, at) > 1.0
    {
        check.since_reset = 0;
        return (check, Some((root_position, root_yaw, root_pitch)));
    }
    (check, None)
}

/// What the end of the player's tick does to the state (`tickPlayer`): a player who
/// drives their root has the checks of their next moves count from where it is now, and
/// is put out for holding it in the air too long (true); one who does not drive is
/// cleared. The age of the last reset grows by one, up to where it stops mattering.
pub(crate) fn end_of_tick(
    check: &VehicleCheck,
    driving: Option<(Vec3, f64)>,
    allow_flight: bool,
) -> (VehicleCheck, bool) {
    let mut check = *check;
    check.since_reset = check
        .since_reset
        .saturating_add(1)
        .min(VehicleCheck::LONG_AGO);
    let Some((position, gravity)) = driving else {
        check.good = None;
        check.floating = false;
        check.floating_ticks = 0;
        return (check, false);
    };
    check = seeded(&check, position);
    if check.floating && !allow_flight {
        check.floating_ticks = check.floating_ticks.saturating_add(1);
        if check.floating_ticks > maximum_flying_ticks(gravity) {
            check.floating = false;
            check.floating_ticks = 0;
            return (check, true);
        }
    } else {
        check.floating = false;
        check.floating_ticks = 0;
    }
    (check, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::entities::Mover;
    use clustine_data::BlockState;
    use clustine_world::{BlockPos, ChunkPos, EntityId};

    /// Air everywhere but a stone floor whose top is y = 0.
    struct Floor;

    impl World for Floor {
        fn block(&self, pos: BlockPos) -> Option<BlockState> {
            Some(if pos.y < 0 {
                clustine_data::blocks::STONE
            } else {
                clustine_data::blocks::AIR
            })
        }

        fn has_chunk(&self, _chunk: ChunkPos) -> bool {
            true
        }
    }

    fn root_at(position: Vec3) -> Root {
        Root {
            body: Body {
                position,
                motion: [0.0; 3],
                on_ground: true,
                fall_distance: 0.0,
                width: 1.375,
                height: 0.5625,
                old_position: position,
                horizontal_collision: false,
                vertical_collision: false,
                vertical_collision_below: true,
                supporting: None,
                on_ground_no_blocks: false,
                stuck_speed: [0.0; 3],
                movements: Vec::new(),
                in_water: false,
                sync_position: false,
                falling_block: false,
                item: false,
                fell_on: None,
                noclip: None,
                mover: Mover {
                    key: EntityId(1),
                    root: EntityId(1),
                    like_a_vehicle: true,
                },
                steps: None,
            },
            yaw: 0.0,
            pitch: 0.0,
            gravity: 0.04,
            bounce: Bounce {
                gravity: 0.04,
                air_drag: 1.0,
                own: 0.0,
                living: false,
                rails_keep_speed: false,
                max_up_step: 0.0,
            },
            passenger: false,
            last_yd: 0.0,
            living: false,
            flying: false,
        }
    }

    fn to(position: Vec3, yaw: f32) -> VehicleMove {
        VehicleMove {
            position: clustine_world::Vec3::new(position[0], position[1], position[2]),
            yaw,
            pitch: 0.0,
            on_ground: true,
        }
    }

    #[test]
    fn a_move_before_the_drivers_first_tick_is_passed_over() {
        let checked = check_move(
            &Floor,
            &VehicleCheck::default(),
            root_at([0.5, 0.0, 0.5]),
            &to([1.5, 0.0, 0.5], 0.0),
            false,
            true,
        );
        assert_eq!(checked.verdict, Verdict::Ignored);
        assert_eq!(checked.correction, None);
    }

    #[test]
    fn a_move_eleven_blocks_away_is_too_quick_unless_the_speed_is_not_tested() {
        let start = [0.5, 0.0, 0.5];
        let check = seeded(&VehicleCheck::default(), start);
        let far = to([11.6, 0.0, 0.5], 0.0);
        let refused = check_move(&Floor, &check, root_at(start), &far, false, true);
        assert_eq!(refused.verdict, Verdict::TooQuickly);
        assert_eq!(refused.correction, Some((start, 0.0, 0.0)));
        let taken = check_move(&Floor, &check, root_at(start), &far, false, false);
        assert_eq!(taken.verdict, Verdict::Accepted);
        assert_eq!(taken.root.body.position, [11.6, 0.0, 0.5]);
        // The next move counts on from the client's place.
        assert_eq!(
            taken.check.good,
            Some([
                [Double(0.5), Double(0.0), Double(0.5)],
                kept([11.6, 0.0, 0.5])
            ])
        );
    }

    #[test]
    fn only_the_horizontal_error_of_a_move_is_judged_and_the_clients_place_is_taken() {
        let start = [0.5, 0.0, 0.5];
        let check = seeded(&VehicleCheck::default(), start);
        // Into the floor by a third of a block: the server's move stops on the floor,
        // but the error along y never counts.
        let sunk = to([0.5, -0.3, 0.5], 0.0);
        let taken = check_move(&Floor, &check, root_at(start), &sunk, false, true);
        // The box below the floor meets the stone it did not meet before.
        assert_eq!(taken.verdict, Verdict::Wrongly);
        // A move along x at the floor is the client's to the bit.
        let along = to([0.75, 0.0, 0.5], 30.0);
        let taken = check_move(&Floor, &check, root_at(start), &along, false, true);
        assert_eq!(taken.verdict, Verdict::Accepted);
        assert_eq!(taken.root.body.position, [0.75, 0.0, 0.5]);
        assert_eq!(taken.root.yaw, 30.0);
    }

    #[test]
    fn a_floating_vehicle_counts_160_ticks_then_puts_its_driver_out_and_ground_clears_it() {
        assert_eq!(maximum_flying_ticks(0.04), 160);
        assert_eq!(maximum_flying_ticks(0.0), u32::MAX);
        let mut check = VehicleCheck {
            floating: true,
            ..VehicleCheck::default()
        };
        let driving = Some(([0.5, 5.0, 0.5], 0.04));
        for tick in 1..=160 {
            let (next, kicked) = end_of_tick(&check, driving, false);
            assert!(!kicked, "at {tick}");
            assert_eq!(next.floating_ticks, tick);
            check = next;
        }
        assert!(end_of_tick(&check, driving, false).1);
        // A tick whose last move was not floating clears the count, and so does not
        // driving.
        let grounded = VehicleCheck {
            floating: false,
            ..check
        };
        assert_eq!(end_of_tick(&grounded, driving, false).0.floating_ticks, 0);
        let off = end_of_tick(&check, None, false).0;
        assert_eq!((off.good, off.floating_ticks), (None, 0));
    }

    #[test]
    fn a_passenger_who_does_not_drive_is_put_back_at_most_every_twenty_ticks_when_near() {
        let root = [0.5, 0.0, 0.5];
        let near = to([0.6, 0.0, 0.5], 0.0);
        let (check, told) = passenger_move(&VehicleCheck::default(), root, 0.0, 0.0, &near);
        assert_eq!(told, Some((root, 0.0, 0.0)));
        assert_eq!(check.since_reset, 0);
        let (_, again) = passenger_move(&check, root, 0.0, 0.0, &near);
        assert_eq!(again, None);
        let far = to([2.0, 0.0, 0.5], 0.0);
        assert!(passenger_move(&check, root, 0.0, 0.0, &far).1.is_some());
        // Twenty-one ticks on, it is put back again.
        let mut aged = check;
        for _ in 0..21 {
            aged = end_of_tick(&aged, None, false).0;
        }
        assert_eq!(aged.since_reset, VehicleCheck::LONG_AGO);
        assert!(passenger_move(&aged, root, 0.0, 0.0, &near).1.is_some());
    }
}
