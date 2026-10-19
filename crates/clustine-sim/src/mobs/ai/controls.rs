//! The controls (the brief's 2.3): `MoveControl` and its flying and swimming kinds,
//! `LookControl` and the swimming one, `JumpControl` and `BodyRotationControl`. They
//! turn the mob's yaw and head and set the inputs `travel` reads (`speed`, `xxa`, `yya`,
//! `zza`, `jumping`); what they want between ticks is the body's
//! [`super::Controls`].

use clustine_blocks::data::in_tag;
use clustine_data::{PathType, attributes};

use super::maths::{
    self, clamp, move_rotlerp, pitch_towards, rotate_if_necessary, rotate_towards, table_cos,
    table_sin, wrap_degrees, yaw_towards,
};
use super::{Ai, LookKind, MoveKind, Operation};
use crate::mobs::tick::MobTick;
use crate::physics::{self, Vec3};

/// `MoveControl.MIN_SPEED_SQR`, as the code compares it in double.
const MIN_SPEED_SQR: f64 = 2.500_000_277_905_201e-7;

/// The epsilon the look controls test a difference against (`1.0E-5f` widened).
const LOOK_EPSILON: f64 = 9.999_999_747_378_752e-6;

/// `MoveControl.setWantedPosition(x, y, z, speed)`: a jump under way keeps going.
pub(crate) fn set_wanted_position(ai: &mut Ai<'_, '_>, wanted: Vec3, speed: f64) {
    let movement = &mut ai.tick.mob.body.controls.movement;
    movement.wanted = wanted;
    movement.speed = speed;
    if movement.operation != Operation::Jumping {
        movement.operation = Operation::MoveTo;
    }
}

/// `MoveControl.strafe(forwards, right)`.
pub(crate) fn strafe(ai: &mut Ai<'_, '_>, forwards: f32, right: f32) {
    let movement = &mut ai.tick.mob.body.controls.movement;
    movement.operation = Operation::Strafe;
    movement.strafe_forwards = forwards;
    movement.strafe_right = right;
    movement.speed = 0.25;
}

/// `MoveControl.setWait()`.
pub(crate) fn set_wait(ai: &mut Ai<'_, '_>) {
    ai.tick.mob.body.controls.movement.operation = Operation::Wait;
}

/// `MoveControl.hasWanted()`.
pub(crate) fn has_wanted(ai: &Ai<'_, '_>) -> bool {
    ai.tick.mob.body.controls.movement.operation == Operation::MoveTo
}

/// `JumpControl.jump()`.
pub(crate) fn jump(ai: &mut Ai<'_, '_>) {
    ai.tick.mob.body.controls.jump = true;
}

/// `Mob.setSpeed(speed)`, which sets `zza` too.
fn set_speed(ai: &mut Ai<'_, '_>, speed: f32) {
    let inputs = &mut ai.tick.mob.body.inputs;
    inputs.speed = speed;
    inputs.zza = speed;
}

/// `LivingEntity.maxUpStep()`: the step height (no player rides a mob of N7's goals).
pub(crate) fn max_up_step(ai: &Ai<'_, '_>) -> f32 {
    ai.attribute(attributes::STEP_HEIGHT) as f32
}

/// `MoveControl.tick()` of the mob's kind.
pub(crate) fn tick_move(ai: &mut Ai<'_, '_>) {
    match ai.config.move_kind {
        MoveKind::Plain => tick_plain_move(ai),
        MoveKind::Flying { max_turn, hovers } => tick_flying_move(ai, max_turn, hovers),
        MoveKind::SmoothSwimming {
            max_turn_x,
            max_turn_y,
            in_water,
            outside_water,
            gravity,
        } => tick_swimming_move(ai, max_turn_x, max_turn_y, in_water, outside_water, gravity),
    }
}

fn movement_speed(ai: &Ai<'_, '_>) -> f64 {
    ai.attribute(attributes::MOVEMENT_SPEED)
}

/// `MoveControl.tick()`.
fn tick_plain_move(ai: &mut Ai<'_, '_>) {
    let state = ai.tick.mob.body.controls.movement.clone();
    match state.operation {
        Operation::Strafe => {
            let base = movement_speed(ai) as f32;
            let speed = state.speed as f32 * base;
            let mut forwards = state.strafe_forwards;
            let mut right = state.strafe_right;
            let mut length = maths::sqrt_f32(forwards * forwards + right * right);
            if length < 1.0 {
                length = 1.0;
            }
            length = speed / length;
            forwards *= length;
            right *= length;
            let sin = table_sin(ai.tick.yaw * maths::RADIANS);
            let cos = table_cos(ai.tick.yaw * maths::RADIANS);
            let dx = forwards * cos - right * sin;
            let dz = right * cos + forwards * sin;
            if !is_walkable(ai, dx, dz) {
                let movement = &mut ai.tick.mob.body.controls.movement;
                movement.strafe_forwards = 1.0;
                movement.strafe_right = 0.0;
            }
            set_speed(ai, speed);
            let movement = ai.tick.mob.body.controls.movement.clone();
            ai.tick.mob.body.inputs.zza = movement.strafe_forwards;
            ai.tick.mob.body.inputs.xxa = movement.strafe_right;
            ai.tick.mob.body.controls.movement.operation = Operation::Wait;
        }
        Operation::MoveTo => {
            ai.tick.mob.body.controls.movement.operation = Operation::Wait;
            let position = ai.position();
            let dx = state.wanted[0] - position[0];
            let dz = state.wanted[2] - position[2];
            let dy = state.wanted[1] - position[1];
            let distance = dx * dx + dy * dy + dz * dz;
            if distance < MIN_SPEED_SQR {
                ai.tick.mob.body.inputs.zza = 0.0;
                return;
            }
            let yaw = yaw_towards(dx, dz);
            ai.tick.yaw = move_rotlerp(ai.tick.yaw, yaw, 90.0);
            let speed = (state.speed * movement_speed(ai)) as f32;
            set_speed(ai, speed);
            let pos = ai.block_position();
            let block = ai.tick.world.state(pos);
            let top = super::pathfinder::shape_top(block, pos);
            let step = f64::from(max_up_step(ai));
            let jumps = (dy > step && dx * dx + dz * dz < f64::from(1.0_f32.max(ai.width())))
                || (top.is_some_and(|top| position[1] < top + f64::from(pos.y))
                    && !in_tag(block, "minecraft:doors")
                    && !in_tag(block, "minecraft:fences"));
            if jumps {
                jump(ai);
                ai.tick.mob.body.controls.movement.operation = Operation::Jumping;
            }
        }
        Operation::Jumping => {
            let speed = (state.speed * movement_speed(ai)) as f32;
            set_speed(ai, speed);
            // Every mob is affected by fluids.
            if ai.on_ground() || ai.in_liquid() {
                ai.tick.mob.body.controls.movement.operation = Operation::Wait;
            }
        }
        Operation::Wait => {
            ai.tick.mob.body.inputs.zza = 0.0;
        }
    }
}

/// `MoveControl.isWalkable(dx, dz)`: the evaluator's type of the cell the strafe would
/// step into is walkable.
fn is_walkable(ai: &Ai<'_, '_>, dx: f32, dz: f32) -> bool {
    let position = ai.position();
    let pos = physics::containing([
        position[0] + f64::from(dx),
        f64::from(ai.block_position().y),
        position[2] + f64::from(dz),
    ]);
    let searcher = super::navigation::searcher(ai);
    super::pathfinder::evaluator_path_type(ai.tick.world, &searcher, pos) == PathType::Walkable
}

/// `FlyingMoveControl.tick()`. **Named**: the flag of no gravity it sets and clears is
/// not kept yet (no flyer is built in this part).
fn tick_flying_move(ai: &mut Ai<'_, '_>, max_turn: i32, hovers: bool) {
    let _ = hovers;
    let state = ai.tick.mob.body.controls.movement.clone();
    if state.operation != Operation::MoveTo {
        ai.tick.mob.body.inputs.yya = 0.0;
        ai.tick.mob.body.inputs.zza = 0.0;
        return;
    }
    ai.tick.mob.body.controls.movement.operation = Operation::Wait;
    let position = ai.position();
    let dx = state.wanted[0] - position[0];
    let dy = state.wanted[1] - position[1];
    let dz = state.wanted[2] - position[2];
    if dx * dx + dy * dy + dz * dz < MIN_SPEED_SQR {
        ai.tick.mob.body.inputs.yya = 0.0;
        ai.tick.mob.body.inputs.zza = 0.0;
        return;
    }
    let yaw = yaw_towards(dx, dz);
    ai.tick.yaw = move_rotlerp(ai.tick.yaw, yaw, 90.0);
    let base = if ai.on_ground() {
        movement_speed(ai)
    } else {
        ai.attribute(attributes::FLYING_SPEED)
    };
    let speed = (state.speed * base) as f32;
    set_speed(ai, speed);
    let horizontal = (dx * dx + dz * dz).sqrt();
    if dy.abs() > LOOK_EPSILON || horizontal.abs() > LOOK_EPSILON {
        let pitch = pitch_towards(dy, horizontal);
        ai.tick.pitch = move_rotlerp(ai.tick.pitch, pitch, max_turn as f32);
        ai.tick.mob.body.inputs.yya = if dy > 0.0 { speed } else { -speed };
    }
}

/// `SmoothSwimmingMoveControl.tick()`.
fn tick_swimming_move(
    ai: &mut Ai<'_, '_>,
    max_turn_x: i32,
    max_turn_y: i32,
    in_water: f32,
    outside_water: f32,
    gravity: bool,
) {
    if gravity && ai.in_water() {
        ai.tick.living.body.motion[1] += 0.005;
    }
    let state = ai.tick.mob.body.controls.movement.clone();
    if state.operation == Operation::MoveTo && !super::navigation::is_done(ai) {
        let position = ai.position();
        let dx = state.wanted[0] - position[0];
        let dy = state.wanted[1] - position[1];
        let dz = state.wanted[2] - position[2];
        if dx * dx + dy * dy + dz * dz < MIN_SPEED_SQR {
            ai.tick.mob.body.inputs.zza = 0.0;
            return;
        }
        let yaw = yaw_towards(dx, dz);
        ai.tick.yaw = move_rotlerp(ai.tick.yaw, yaw, max_turn_y as f32);
        ai.tick.mob.body.head.body_yaw = ai.tick.yaw;
        ai.tick.mob.body.head.head_yaw = ai.tick.yaw;
        let speed = (state.speed * movement_speed(ai)) as f32;
        if ai.in_water() {
            set_speed(ai, speed * in_water);
            let horizontal = (dx * dx + dz * dz).sqrt();
            if dy.abs() > LOOK_EPSILON || horizontal.abs() > LOOK_EPSILON {
                let pitch = pitch_towards(dy, horizontal);
                let pitch = clamp(wrap_degrees(pitch), -(max_turn_x as f32), max_turn_x as f32);
                ai.tick.pitch = rotate_towards(ai.tick.pitch, pitch, 5.0);
            }
            let radians = ai.tick.pitch * maths::RADIANS;
            ai.tick.mob.body.inputs.zza = table_cos(radians) * speed;
            ai.tick.mob.body.inputs.yya = -table_sin(radians) * speed;
        } else {
            let turn = (wrap_degrees(ai.tick.yaw - yaw)).abs();
            let slowed = 1.0 - clamp((turn - 10.0) / 50.0, 0.0, 1.0);
            set_speed(ai, speed * outside_water * slowed);
        }
    } else {
        set_speed(ai, 0.0);
        let inputs = &mut ai.tick.mob.body.inputs;
        inputs.xxa = 0.0;
        inputs.yya = 0.0;
        inputs.zza = 0.0;
    }
}

/// `LookControl.setLookAt(x, y, z, yawSpeed, maxPitch)`.
pub(crate) fn set_look_at(ai: &mut Ai<'_, '_>, wanted: Vec3, yaw_speed: f32, max_pitch: f32) {
    let look = &mut ai.tick.mob.body.controls.look;
    look.wanted = wanted;
    look.y_max_rot_speed = yaw_speed;
    look.x_max_rot_angle = max_pitch;
    look.cooldown = 2;
}

/// `LookControl.setLookAt(x, y, z)`: at the head's own speed and pitch.
pub(crate) fn set_look_at_default(ai: &mut Ai<'_, '_>, wanted: Vec3) {
    let head = ai.head();
    set_look_at(ai, wanted, head.rot_speed as f32, head.max_x_rot as f32);
}

/// `LookControl.isLookingAtTarget()`.
pub(crate) fn is_looking(ai: &Ai<'_, '_>) -> bool {
    ai.tick.mob.body.controls.look.cooldown > 0
}

/// `getYRotD()`: the yaw that faces the wanted point, none when it is straight above
/// or below.
fn wanted_yaw(ai: &Ai<'_, '_>) -> Option<f32> {
    let wanted = ai.tick.mob.body.controls.look.wanted;
    let position = ai.position();
    let dx = wanted[0] - position[0];
    let dz = wanted[2] - position[2];
    (dz.abs() > LOOK_EPSILON || dx.abs() > LOOK_EPSILON).then(|| yaw_towards(dx, dz))
}

/// `getXRotD()`: the pitch that looks at the wanted point from the eyes.
fn wanted_pitch(ai: &Ai<'_, '_>) -> Option<f32> {
    let wanted = ai.tick.mob.body.controls.look.wanted;
    let position = ai.position();
    let dx = wanted[0] - position[0];
    let dy = wanted[1] - ai.eye_y();
    let dz = wanted[2] - position[2];
    let horizontal = (dx * dx + dz * dz).sqrt();
    (dy.abs() > LOOK_EPSILON || horizontal.abs() > LOOK_EPSILON)
        .then(|| pitch_towards(dy, horizontal))
}

/// `LookControl.tick()` of the mob's kind.
pub(crate) fn tick_look(ai: &mut Ai<'_, '_>) {
    match ai.config.look_kind {
        LookKind::Plain => tick_plain_look(ai),
        LookKind::SmoothSwimming {
            max_y_rot_from_center,
        } => tick_swimming_look(ai, max_y_rot_from_center),
    }
}

/// `LookControl.tick()`: the pitch put back to nothing, then turned only while looking;
/// the head towards the wanted yaw, or back towards the body; kept within reach of the
/// body while the mob paths.
fn tick_plain_look(ai: &mut Ai<'_, '_>) {
    // `resetXRotOnTick`.
    ai.tick.pitch = 0.0;
    let look = ai.tick.mob.body.controls.look.clone();
    if look.cooldown > 0 {
        ai.tick.mob.body.controls.look.cooldown -= 1;
        if let Some(yaw) = wanted_yaw(ai) {
            let head = &mut ai.tick.mob.body.head;
            head.head_yaw = rotate_towards(head.head_yaw, yaw, look.y_max_rot_speed);
        }
        if let Some(pitch) = wanted_pitch(ai) {
            ai.tick.pitch = rotate_towards(ai.tick.pitch, pitch, look.x_max_rot_angle);
        }
    } else {
        let head = &mut ai.tick.mob.body.head;
        head.head_yaw = rotate_towards(head.head_yaw, head.body_yaw, 10.0);
    }
    clamp_head_to_body(ai);
}

/// `LookControl.clampHeadRotationToBody()`: only while the mob paths.
fn clamp_head_to_body(ai: &mut Ai<'_, '_>) {
    if !super::navigation::is_done(ai) {
        let max = ai.head().max_y_rot as f32;
        let head = &mut ai.tick.mob.body.head;
        head.head_yaw = rotate_if_necessary(head.head_yaw, head.body_yaw, max);
    }
}

/// `SmoothSwimmingLookControl.tick()`.
fn tick_swimming_look(ai: &mut Ai<'_, '_>, max_y_rot_from_center: i32) {
    let look = ai.tick.mob.body.controls.look.clone();
    if look.cooldown > 0 {
        ai.tick.mob.body.controls.look.cooldown -= 1;
        if let Some(yaw) = wanted_yaw(ai) {
            let head = &mut ai.tick.mob.body.head;
            head.head_yaw = rotate_towards(head.head_yaw, yaw + 20.0, look.y_max_rot_speed);
        }
        if let Some(pitch) = wanted_pitch(ai) {
            ai.tick.pitch = rotate_towards(ai.tick.pitch, pitch + 10.0, look.x_max_rot_angle);
        }
    } else {
        if super::navigation::is_done(ai) {
            ai.tick.pitch = rotate_towards(ai.tick.pitch, 0.0, 5.0);
        }
        let head = &mut ai.tick.mob.body.head;
        head.head_yaw = rotate_towards(head.head_yaw, head.body_yaw, look.y_max_rot_speed);
    }
    let head = &mut ai.tick.mob.body.head;
    let off = wrap_degrees(head.head_yaw - head.body_yaw);
    let max = max_y_rot_from_center as f32;
    if off < -max {
        head.body_yaw -= 4.0;
    } else if off > max {
        head.body_yaw += 4.0;
    }
}

/// `JumpControl.tick()`: the jump asked for in this step, for `aiStep`'s jump.
pub(crate) fn tick_jump(ai: &mut Ai<'_, '_>) {
    let controls = &mut ai.tick.mob.body.controls;
    ai.tick.mob.body.inputs.jumping = controls.jump;
    controls.jump = false;
}

/// `BodyRotationControl.clientTick()`, after `aiStep`: a moving body faces where it
/// goes; a still one turns to its head after ten still ticks, over ten more. The game's
/// `headStableTime` is the tick count from `Head::still_since`: after this tick it is
/// `lived + 1 - still_since`, a reset setting `still_since` to `lived + 1`.
pub(crate) fn rotate_body(tick: &mut MobTick<'_>) {
    let max = tick.mob_type.head().max_y_rot as f32;
    let body = &tick.living.body;
    let moved = {
        let dx = body.position[0] - body.old_position[0];
        let dz = body.position[2] - body.old_position[2];
        dx * dx + dz * dz > MIN_SPEED_SQR
    };
    let next = tick.lived.saturating_add(1);
    let yaw = tick.yaw;
    let head = &mut tick.mob.body.head;
    let controls = &mut tick.mob.body.controls;
    if moved {
        head.body_yaw = yaw;
        head.head_yaw = rotate_if_necessary(head.head_yaw, head.body_yaw, max);
        controls.last_stable_head_yaw = head.head_yaw;
        head.still_since = next;
        return;
    }
    // **Named**: a mob carrying a mob does not count its stillness in the game; the
    // region does not yet tell a tick what its mob carries, and none of N7's does.
    if (head.head_yaw - controls.last_stable_head_yaw).abs() > 15.0 {
        head.still_since = next;
        controls.last_stable_head_yaw = head.head_yaw;
        head.body_yaw = rotate_if_necessary(head.body_yaw, head.head_yaw, max);
    } else {
        let stable = next.wrapping_sub(head.still_since) as i32;
        if stable > 10 {
            let fraction = clamp((stable - 10) as f32 / 10.0, 0.0, 1.0);
            head.body_yaw =
                rotate_if_necessary(head.body_yaw, head.head_yaw, max * (1.0 - fraction));
        }
    }
}
