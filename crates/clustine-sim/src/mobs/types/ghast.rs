//! The ghast (`Ghast` and its inner classes, d javap; `docs/groundwork/p-brief.md`,
//! section 5.9; family E2 of P12, `docs/groundwork/p-plan.md`, sections 18.3, 18.5 and
//! 18.6): made by natural spawning where the Nether's lists name it, and by its egg.
//!
//! It flies (`travelFlying` at 0.02 in the air, in water and in lava alike) where its
//! move control pushes it (`Ghast$GhastMoveControl`, not careful and never stopped):
//! moving to a point, every 2 to 6 ticks an impulse of five thirds of its flying speed
//! towards it where its box can sweep there, else it waits. Its goals (`registerGoals`,
//! Ghast.txt 0–78): it floats to a random point within 16 each way where it wants none
//! or the one it wants is nearer than a block or further than 60
//! (`RandomFloatAroundGoal`, of no distance to blocks); it faces its target within 64,
//! else the way it drifts (`GhastLookGoal`, `faceMovementDirection`); and it shoots
//! (`GhastShootFireballGoal`): with its target within 64 and in sight its charge counts
//! up, at 10 level event 1015 (its warning), at 20 level event 1016 and a large fireball
//! from four blocks ahead of it along its look, aimed at the target's middle, at its
//! explosion power; then the charge is −40. Out of reach or sight, the charge counts
//! down to nothing. It shows itself charging above 10 (`DATA_IS_CHARGING`). Its target is
//! the nearest player it sees within 4 up or down (`NearestAttackableTargetGoal`, every
//! player being creative it finds none until survival).
//!
//! It is immune to fire, and so to its own fireballs, but a large fireball a player sent
//! back hurts it by 1000 whatever was dealt (`hurtServer`, `isInvulnerableTo`). It
//! climbs nothing and takes no fall. Its sounds are loud (5) and hostile; it gives 5
//! experience; its loot is its table's (a tear, gunpowder, and the disc where a player's
//! fireball killed it). Its spawn rule: not peaceful, one in twenty, and the mob rules.
//!
//! Named differences: its fall's landing effects are not left out (the game's
//! `checkFallDamage` does nothing; a ghast is in `#fall_damage_immune` and flies, so only
//! a landing's particles and sound could show); its move control's `floatDuration`,
//! which the game keeps in memory, is its type state, so a ghast loaded again keeps it.

use clustine_data::{attributes, entity_types, sounds};
use clustine_world::{BlockPos, Difficulty, TypeState};
use serde::{Deserialize, Serialize};

use super::MobType;
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Looked};
use crate::mobs::ai::targets::{self, NearestTarget, Target};
use crate::mobs::ai::{Ai, Operation, Setup, controls, maths};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::Local;
use crate::physics::aabb::Aabb;
use crate::physics::collide::{self, Context};
use crate::physics::{self, Vec3, walk};
use crate::region::Random;
use crate::shown::DataShown;
use crate::spawning::rules::SpawnWorld;
use clustine_world::generation::Draws;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The ghast's entity type, by id.
pub(crate) const GHAST: u16 = entity_types::GHAST as u16;

/// The version of [`GhastState`]'s encoding.
const VERSION: u8 = 1;

/// `DATA_IS_CHARGING`'s index, the first after `Mob`'s.
pub(crate) const CHARGING_INDEX: u8 = 16;

/// `DEFAULT_EXPLOSION_POWER`.
pub(crate) const DEFAULT_EXPLOSION_POWER: u8 = 1;

/// The level events of its charge: `GHAST_WARN` at 10, `GHAST_SHOOT` at 20.
pub(crate) const WARN_EVENT: i32 = 1015;
pub(crate) const SHOOT_EVENT: i32 = 1016;

/// `RandomFloatAroundGoal`'s reach about where the ghast is.
const FLOAT_REACH: f32 = 16.0;

/// How far the ghast faces and shoots at its target: 64, squared.
const REACH_SQR: f64 = 4096.0;

/// What a ghast keeps: its synced `DATA_IS_CHARGING`, its saved `ExplosionPower`, and
/// its move control's `floatDuration`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GhastState {
    pub(crate) charging: bool,
    pub(crate) explosion_power: u8,
    pub(crate) float_duration: i32,
}

impl Default for GhastState {
    fn default() -> Self {
        Self {
            charging: false,
            explosion_power: DEFAULT_EXPLOSION_POWER,
            float_duration: 0,
        }
    }
}

/// A ghast's state from its type state; nothing, or what this build cannot read, as a
/// fresh ghast's.
pub(crate) fn state_of(mob: &Mob) -> GhastState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return GhastState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// The state changed by `change`, written back where it changed.
pub(crate) fn change(mob: &mut Mob, change: impl FnOnce(&mut GhastState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = if own == GhastState::default() {
            TypeState::default()
        } else {
            TypeState {
                version: VERSION,
                bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
            }
        };
    }
}

/// The ghast's target goal's selector (`registerGoals`' lambda, Ghast.txt 0–24): within
/// 4 up or down.
fn within_four_up_or_down(ai: &Ai<'_, '_>, target: &Looked) -> bool {
    (target.position[1] - ai.position()[1]).abs() <= 4.0
}

/// `Ghast.registerGoals`' target: `NearestAttackableTargetGoal(this, Player, 10, true,
/// false, …)`.
const TARGETS: &[(i32, Target)] = &[(
    1,
    Target::Nearest(NearestTarget {
        selector: Some(within_four_up_or_down),
        ..NearestTarget::players(true)
    }),
)];

/// `Ghast$RandomFloatAroundGoal(ghast)`: of no distance to blocks, so its first point
/// within its home (if it has one) is good.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RandomFloatAroundGoal;

impl RandomFloatAroundGoal {
    /// `chooseRandomPosition(pos, random)`: up to 16 each way, x, then y, then z.
    fn random_position(ai: &mut Ai<'_, '_>, from: Vec3) -> Vec3 {
        let mut part = || f64::from((ai.tick.draws.next_float() * 2.0 - 1.0) * FLOAT_REACH);
        let x = from[0] + part();
        let y = from[1] + part();
        let z = from[2] + part();
        [x, y, z]
    }

    /// `getSuitableFlyToPosition(ghast, 0)`: up to 64 points, the first within its home
    /// good (`isGoodTarget` of no distance passes every point); none of them within it,
    /// one more drawn, brought down below the surface's height where it is above it.
    fn suitable_position(ai: &mut Ai<'_, '_>) -> Vec3 {
        let from = ai.position();
        for _ in 0..64 {
            let chosen = Self::random_position(ai, from);
            if ai.home().is_none() || ai.within_home(physics::containing(chosen)) {
                return chosen;
            }
        }
        let mut chosen = Self::random_position(ai, from);
        let at = physics::containing(chosen);
        if let Some(height) = ai.tick.world.motion_blocking_height(at.x, at.z)
            && height < at.y
            && height > ai.tick.world.min_y()
        {
            let y = ai.position()[1];
            chosen = [chosen[0], y - (y - chosen[1]).abs(), chosen[2]];
        }
        chosen
    }
}

impl Goal for RandomFloatAroundGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    /// `canUse()`: no point wanted, or one nearer than a block or further than 60.
    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let movement = &ai.tick.mob.body.controls.movement;
        if movement.operation != Operation::MoveTo {
            return true;
        }
        let distance = physics::distance_sqr(movement.wanted, ai.position());
        !(1.0..=3600.0).contains(&distance)
    }

    fn can_continue(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let wanted = Self::suitable_position(ai);
        controls::set_wanted_position(ai, wanted, 1.0);
    }
}

/// `Entity.calculateViewVector(xRot, yRot)` of `yaw` and `pitch` in degrees, by the
/// game's table of sines, in floats: where a deflection sends a fireball, and where a
/// ghast looks as it shoots.
pub(crate) fn view_vector(yaw: f32, pitch: f32) -> Vec3 {
    use clustine_noise::trig;
    let pitch = pitch * 0.017_453_292;
    let yaw = -yaw * 0.017_453_292;
    let yaw_cos = trig::cos(f64::from(yaw));
    let yaw_sin = trig::sin(f64::from(yaw));
    let pitch_cos = trig::cos(f64::from(pitch));
    let pitch_sin = trig::sin(f64::from(pitch));
    [
        f64::from(yaw_sin * pitch_cos),
        f64::from(-pitch_sin),
        f64::from(yaw_cos * pitch_cos),
    ]
}

/// `Ghast.faceMovementDirection(mob)` (Ghast.txt 246–331): with no target, its yaw and
/// body the way it drifts; with one within 64, towards it; else as they are.
pub(crate) fn face_movement_direction(ai: &mut Ai<'_, '_>) {
    let yaw = match sensing::target(ai) {
        None => {
            let motion = ai.tick.living.body.motion;
            -(maths::atan2(motion[0], motion[2]) as f32) * 57.295_776
        }
        Some(target) => {
            let [x, _, z] = ai.position();
            if physics::distance_sqr(target.position, ai.position()) >= REACH_SQR {
                return;
            }
            let (dx, dz) = (target.position[0] - x, target.position[2] - z);
            -(maths::atan2(dx, dz) as f32) * 57.295_776
        }
    };
    ai.tick.yaw = yaw;
    ai.tick.mob.body.head.body_yaw = yaw;
}

/// `Ghast$GhastLookGoal`: every tick, `faceMovementDirection`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GhastLookGoal;

impl Goal for GhastLookGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        true
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        face_movement_direction(ai);
    }
}

/// `GhastShootFireballGoal`'s state: `chargeTime`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Charging {
    pub(crate) charge_time: i32,
}

/// `Ghast$GhastShootFireballGoal` (its `tick`, 0–369): of no flags, every tick, while the
/// ghast has a target.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GhastShootFireballGoal;

impl GhastShootFireballGoal {
    /// The large fireball at the charge's twentieth tick: from four blocks ahead of the
    /// ghast along its look (`getViewVector(1)`, of its pitch and its head's yaw) and half
    /// a block above its middle, towards the target's middle.
    fn shoot(ai: &mut Ai<'_, '_>, target: &Looked) {
        let view = view_vector(ai.tick.mob.body.head.head_yaw, ai.tick.pitch);
        let [x, y, z] = ai.position();
        // `getY(0.5)`: the feet and half the height, of the float height widened.
        let middle = y + f64::from(ai.height()) * 0.5;
        let target_height = (target.bounds.max[1] - target.bounds.min[1]) as f32;
        let target_middle = target.position[1] + f64::from(target_height) * 0.5;
        let dx = target.position[0] - (x + view[0] * 4.0);
        let dy = target_middle - (0.5 + middle);
        let dz = target.position[2] - (z + view[2] * 4.0);
        ai.tick.out.local.push(Local::LevelEvent {
            kind: SHOOT_EVENT,
            pos: ai.block_position(),
            data: 0,
        });
        let direction = physics::normalize([dx, dy, dz]);
        let at = [x + view[0] * 4.0, middle + 0.5, z + view[2] * 4.0];
        let explosion_power = state_of(&ai.tick.mob).explosion_power;
        ai.tick.out.local.push(Local::ShootFireball {
            kind: entity_types::FIREBALL as u16,
            at,
            direction,
            explosion_power,
        });
    }
}

impl Goal for GhastShootFireballGoal {
    type State = Charging;

    fn flags(&self) -> u8 {
        0
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut Charging, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some()
    }

    fn start(&self, state: &mut Charging, _: &mut Ai<'_, '_>) {
        state.charge_time = 0;
    }

    fn stop(&self, _: &mut Charging, ai: &mut Ai<'_, '_>) {
        change(&mut ai.tick.mob, |own| own.charging = false);
    }

    fn tick(&self, state: &mut Charging, ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        let near = target.distance_sqr(ai.position()) < REACH_SQR;
        if near && sensing::has_line_of_sight(ai, &target) {
            state.charge_time += 1;
            if state.charge_time == 10 {
                ai.tick.out.local.push(Local::LevelEvent {
                    kind: WARN_EVENT,
                    pos: ai.block_position(),
                    data: 0,
                });
            }
            if state.charge_time == 20 {
                Self::shoot(ai, &target);
                state.charge_time = -40;
            }
        } else if state.charge_time > 0 {
            state.charge_time -= 1;
        }
        let charging = state.charge_time > 10;
        change(&mut ai.tick.mob, |own| own.charging = charging);
    }
}

/// `CollisionContext.empty()`, which `getCollisionShape(level, pos)` uses.
const NO_ENTITY: Context = Context {
    stands_on_lava: false,
    bottom: -f64::MAX,
    fall_distance: 0.0,
    falling_block: false,
    noclip: None,
};

/// `GhastMoveControl.blockTraversalPossible` of a ghast, which is not careful, for a
/// move from `from` to `to`: air passes; else whether its box moving so misses the
/// block's collision shape. A block of a chunk the region does not read is unknown, and
/// does not pass.
fn traversal_possible(ai: &Ai<'_, '_>, from: Vec3, to: Vec3, pos: BlockPos) -> bool {
    let world = ai.tick.world;
    if !world.has_chunk(pos.chunk()) {
        return false;
    }
    let state = world.state(pos);
    if state.props().is_air() {
        return true;
    }
    let boxes = collide::collision_shape(state, pos, NO_ENTITY)
        .map(|(shape, _)| shape.boxes())
        .unwrap_or_default();
    let moving = Aabb::of_entity(from, ai.width(), ai.height());
    !moving.collided_along_vector(physics::sub(to, from), &boxes)
}

/// `GhastMoveControl.canReach(delta)` of a ghast: every block its box sweeps through on
/// the way that it is not in already passes.
fn can_reach(ai: &Ai<'_, '_>, delta: Vec3) -> bool {
    let position = ai.position();
    let current = Aabb::of_entity(position, ai.width(), ai.height());
    let moved = current.moved(delta);
    let to = physics::add(position, delta);
    walk::for_each_block_between(position, to, &moved, |pos, _| {
        current.intersects(Aabb::of_block(pos)) || traversal_possible(ai, position, to, pos)
    })
}

/// `Ghast$GhastMoveControl.tick()` of a ghast (never stopped): moving to a point, as its
/// `floatDuration` runs out (counted down every such tick), another 2 to 6 on it (one
/// draw) and an impulse of five thirds of its flying speed towards the point where it can
/// reach it, else it waits.
pub(crate) fn tick_move_control(ai: &mut Ai<'_, '_>) {
    let movement = ai.tick.mob.body.controls.movement.clone();
    if movement.operation != Operation::MoveTo {
        return;
    }
    let old = state_of(&ai.tick.mob).float_duration;
    let mut duration = old - 1;
    if old <= 0 {
        duration += ai.tick.draws.next_int(5) as i32 + 2;
        let delta = physics::sub(movement.wanted, ai.position());
        if can_reach(ai, delta) {
            let speed = ai.attribute(attributes::FLYING_SPEED) * 5.0 / 3.0;
            let push = physics::scale(physics::normalize(delta), speed);
            let body = &mut ai.tick.living.body;
            body.motion = physics::add(body.motion, push);
        } else {
            ai.tick.mob.body.controls.movement.operation = Operation::Wait;
        }
    }
    change(&mut ai.tick.mob, |own| own.float_duration = duration);
}

/// The ghast.
pub(crate) struct Ghast;

impl MobType for Ghast {
    /// `Ghast.registerGoals` (Ghast.txt 0–78).
    fn register_goals(&self, setup: &mut Setup) {
        setup.add_goal(5, RandomFloatAroundGoal);
        setup.add_goal(7, GhastLookGoal);
        setup.add_goal(7, GhastShootFireballGoal);
        targets::register(setup, TARGETS);
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        tick_move_control(ai);
    }

    /// `travel(input)`: `travelFlying(input, 0.02f)`.
    fn travel(&self, _: &crate::mobs::tick::MobTick<'_>) -> Travel {
        Travel::Flying {
            water: 0.02,
            lava: 0.02,
            air: 0.02,
        }
    }

    fn on_climbable(&self, _: &Mob) -> Option<bool> {
        Some(false)
    }

    /// `isReflectedFireball(source)`: a large fireball whose cause is a player hurts it
    /// by 1000, whatever it is immune to.
    fn hurt_overriding(&self, source: &DamageSource) -> Option<f32> {
        let reflected = source
            .direct
            .as_ref()
            .is_some_and(|direct| i32::from(direct.kind) == entity_types::FIREBALL)
            && source
                .cause
                .as_ref()
                .is_some_and(|cause| cause.player.is_some());
        reflected.then_some(1000.0)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_GHAST_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_GHAST_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_GHAST_DEATH)
    }

    /// `getSoundVolume()`.
    fn sound_volume(&self) -> f32 {
        5.0
    }

    /// `xpReward` 5 (`Mob.getBaseExperienceReward`), with what its equipment adds.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(5, mob, draws)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let charging = state_of(mob).charging;
        vec![(CHARGING_INDEX, DataShown::Boolean(charging), !charging)]
    }
}

/// `Ghast.checkGhastSpawnRules` (Ghast.txt 235–261), which `spawning::rules` asks: not
/// peaceful, then one draw in twenty, then the mob rules.
pub(crate) fn check_spawn_rules(
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    Difficulty::WORLD != Difficulty::Peaceful
        && draws.next_int(20) == 0
        && crate::spawning::rules::check_mob_spawn_rules(GHAST, world, pos)
}

#[cfg(test)]
#[path = "ghast_tests.rs"]
mod tests;
