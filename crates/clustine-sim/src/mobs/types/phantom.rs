//! The phantom (`Phantom` and its inner classes; `docs/groundwork/n9-plan.md`, section
//! 8.4; n9 Phantom): made by its spawner (part S11's, `region/custom_spawners.rs`, which
//! sizes it with [`set_size`]) and by its egg.
//!
//! It flies (`travelFlying` at 0.2 in the air, no gravity) where its move control steers
//! it: towards its move target, turning at most 4 a tick, its speed easing towards 1.8
//! while it flies straight and towards 0.2 while it turns, its motion a fifth of the way
//! to that each tick; turned about where it ran into something. Its look control does
//! nothing, and its body faces its yaw (its head its body's of the tick before). It
//! circles about its anchor (`PhantomCircleAroundAnchorGoal`: 5 to 15 away, −4 to 5 up,
//! one way or the other, its angle 15 degrees on at each new point), its anchor five
//! blocks above where it was made. It looks for a player to swoop at every 30 of its goal
//! ticks (`PhantomAttackPlayerTargetGoal`, by combat conditions, which refuse every
//! player, all being creative), so it circles for ever; its attack strategy and sweep,
//! which only a target starts, are built as the game's for a target it cannot get. It
//! burns in daylight (`#burn_in_daylight`, the body's `burn_undead`), climbs nothing and
//! takes no fall.
//!
//! What its goals share (the move target, the anchor, the attack's phase) and its move
//! control's speed are its type state, which the game keeps in memory (named: a phantom
//! loaded again keeps them, where the game's starts its move target at nought and its
//! speed at 0.1).
//!
//! Named differences: its flaps' sounds and particles are the client's; the cats a sweep
//! makes hiss do not (a cat's own draws), though they still scare it off; `wasHurtRecently`
//! is read as its hurt time running.

use clustine_data::{EntityTypeInfo, attributes, entity_types, sounds};
use clustine_world::{BlockPos, TypeState};
use serde::{Deserialize, Serialize};

use super::{Dimensions, MobType, Spawning};
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::maths::{self, reduced_tick_delay, wrap_degrees};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Conditions};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::{Event, attribute};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The version of a phantom's type state.
const VERSION: u8 = 1;

/// `ID_SIZE`'s index.
const SIZE_INDEX: u8 = 16;

/// The overworld's sea level, below which no anchor goes.
const SEA_LEVEL: i32 = 63;

/// `Phantom.AttackPhase`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AttackPhase {
    #[default]
    Circle,
    Swoop,
}

/// What a phantom keeps: its size, its anchor, its move target, its attack's phase, and
/// its move control's speed (none before its first move, the game's 0.1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PhantomState {
    pub(crate) size: i32,
    pub(crate) anchor: Option<BlockPos>,
    pub(crate) move_target: Vec3,
    pub(crate) phase: AttackPhase,
    pub(crate) speed: Option<f32>,
}

/// A phantom's state from its type state; a record of nothing, or one this build cannot
/// read, as a fresh phantom's.
pub(crate) fn state_of(mob: &Mob) -> PhantomState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return PhantomState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// The state changed by `change`, written back where it changed.
pub(crate) fn change(mob: &mut Mob, change: impl FnOnce(&mut PhantomState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = if own == PhantomState::default() {
            TypeState::default()
        } else {
            TypeState {
                version: VERSION,
                bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
            }
        };
    }
}

const fn kind() -> u16 {
    entity_types::PHANTOM as u16
}

/// `setPhantomSize(size)`: held to 0 to 64; where it changes, its attack `6 + size`
/// (`updatePhantomSizeInfo`, which only a change of the synced value runs).
pub(crate) fn set_size(mob: &mut Mob, size: i32) {
    let size = size.clamp(0, 64);
    if state_of(mob).size == size {
        return;
    }
    change(mob, |own| own.size = size);
    if let Some(attack) = mob
        .body
        .attributes
        .instance(kind(), attribute(attributes::ATTACK_DAMAGE))
    {
        attack.base = f64::from(6 + size);
    }
}

/// `Mth.approach(value, target, maxDelta)`.
fn approach(value: f32, target: f32, max_delta: f32) -> f32 {
    let delta = max_delta.abs();
    if value < target {
        maths::clamp(value + delta, value, target)
    } else {
        maths::clamp(value - delta, target, value)
    }
}

/// `Mth.approachDegrees(current, target, maxDelta)`.
fn approach_degrees(current: f32, target: f32, max_delta: f32) -> f32 {
    let difference = wrap_degrees(target - current);
    approach(current, current + difference, max_delta)
}

/// `PhantomMoveControl.tick()`.
pub(crate) fn tick_move_control(ai: &mut Ai<'_, '_>) {
    let own = state_of(&ai.tick.mob);
    let mut speed = own.speed.unwrap_or(0.1);
    if ai.tick.living.body.horizontal_collision {
        ai.tick.yaw += 180.0;
        speed = 0.1;
    }
    let [x, y, z] = ai.position();
    let mut dx = own.move_target[0] - x;
    let dy = own.move_target[1] - y;
    let mut dz = own.move_target[2] - z;
    let mut horizontal = (dx * dx + dz * dz).sqrt();
    if horizontal.abs() > 9.999_999_747_378_752e-6 {
        let factor = 1.0 - (dy * 0.699_999_988_079_071).abs() / horizontal;
        dx *= factor;
        dz *= factor;
        horizontal = (dx * dx + dz * dz).sqrt();
        let distance = (dx * dx + dz * dz + dy * dy).sqrt();
        let old_yaw = ai.tick.yaw;
        let angle = maths::atan2(dz, dx) as f32;
        let current = wrap_degrees(ai.tick.yaw + 90.0);
        let wanted = wrap_degrees(angle * 57.295_776);
        ai.tick.yaw = approach_degrees(current, wanted, 4.0) - 90.0;
        ai.tick.mob.body.head.body_yaw = ai.tick.yaw;
        speed = if wrap_degrees(ai.tick.yaw - old_yaw).abs() < 3.0 {
            approach(speed, 1.8, 0.005 * (1.8 / speed))
        } else {
            approach(speed, 0.2, 0.025)
        };
        let pitch = (-(maths::atan2(-dy, horizontal) * 57.295_776_367_187_5)) as f32;
        ai.tick.pitch = pitch;
        let heading = ai.tick.yaw + 90.0;
        let cos = |radians: f32| clustine_noise::trig::cos(f64::from(radians));
        let sin = |radians: f32| clustine_noise::trig::sin(f64::from(radians));
        let vx = f64::from(speed * cos(heading * 0.017_453_292)) * (dx / distance).abs();
        let vz = f64::from(speed * sin(heading * 0.017_453_292)) * (dz / distance).abs();
        let vy = f64::from(speed * sin(pitch * 0.017_453_292)) * (dy / distance).abs();
        let motion = ai.tick.living.body.motion;
        ai.tick.living.body.motion = physics::add(
            motion,
            physics::scale(physics::sub([vx, vy, vz], motion), 0.2),
        );
    }
    if own.speed != Some(speed) {
        change(&mut ai.tick.mob, |own| own.speed = Some(speed));
    }
}

/// `Phantom.registerGoals`.
fn register_goals(setup: &mut Setup) {
    setup.add_goal(1, PhantomAttackStrategyGoal);
    setup.add_goal(2, PhantomSweepAttackGoal);
    setup.add_goal(3, PhantomCircleAroundAnchorGoal);
    setup.add_target(1, PhantomAttackPlayerTargetGoal);
}

/// `PhantomMoveTargetGoal.touchingTarget()`: within 2 of its move target.
fn touching_target(ai: &Ai<'_, '_>) -> bool {
    physics::distance_sqr(state_of(&ai.tick.mob).move_target, ai.position()) < 4.0
}

/// `PhantomCircleAroundAnchorGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Circling {
    pub(crate) angle: f32,
    pub(crate) distance: f32,
    pub(crate) height: f32,
    pub(crate) clockwise: f32,
}

/// `PhantomCircleAroundAnchorGoal`: with no target, or circling, round its anchor.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhantomCircleAroundAnchorGoal;

impl PhantomCircleAroundAnchorGoal {
    /// `selectNext()`: an anchor where it is if it has none; its angle on by 15 degrees
    /// its way; its move target that far round the anchor, at its height less 4.
    fn select_next(state: &mut Circling, ai: &mut Ai<'_, '_>) {
        let block = ai.block_position();
        let anchor = state_of(&ai.tick.mob).anchor.unwrap_or(block);
        state.angle += state.clockwise * 15.0 * 0.017_453_292;
        let cos = clustine_noise::trig::cos(f64::from(state.angle));
        let sin = clustine_noise::trig::sin(f64::from(state.angle));
        let target = [
            f64::from(anchor.x) + f64::from(state.distance * cos),
            f64::from(anchor.y) + f64::from(-4.0 + state.height),
            f64::from(anchor.z) + f64::from(state.distance * sin),
        ];
        change(&mut ai.tick.mob, |own| {
            own.anchor = Some(anchor);
            own.move_target = target;
        });
    }
}

impl Goal for PhantomCircleAroundAnchorGoal {
    type State = Circling;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut Circling, ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.mob.body.combat.target.is_none()
            || state_of(&ai.tick.mob).phase == AttackPhase::Circle
    }

    fn start(&self, state: &mut Circling, ai: &mut Ai<'_, '_>) {
        state.distance = 5.0 + ai.tick.draws.next_float() * 10.0;
        state.height = -4.0 + ai.tick.draws.next_float() * 9.0;
        state.clockwise = if ai.tick.draws.next_bool() { 1.0 } else { -1.0 };
        Self::select_next(state, ai);
    }

    fn tick(&self, state: &mut Circling, ai: &mut Ai<'_, '_>) {
        if ai.tick.draws.next_int(self.adjusted_tick_delay(350) as u32) == 0 {
            state.height = -4.0 + ai.tick.draws.next_float() * 9.0;
        }
        if ai.tick.draws.next_int(self.adjusted_tick_delay(250) as u32) == 0 {
            state.distance += 1.0;
            if state.distance > 15.0 {
                state.distance = 5.0;
                state.clockwise = -state.clockwise;
            }
        }
        if ai.tick.draws.next_int(self.adjusted_tick_delay(450) as u32) == 0 {
            state.angle = ai.tick.draws.next_float() * 2.0 * std::f32::consts::PI;
            Self::select_next(state, ai);
        }
        if touching_target(ai) {
            Self::select_next(state, ai);
        }
        let block = ai.block_position();
        let target_y = state_of(&ai.tick.mob).move_target[1];
        let y = ai.position()[1];
        let below = BlockPos::new(block.x, block.y - 1, block.z);
        if target_y < y && !ai.tick.world.state(below).is_air() {
            state.height = state.height.max(1.0);
            Self::select_next(state, ai);
        }
        let target_y = state_of(&ai.tick.mob).move_target[1];
        let above = BlockPos::new(block.x, block.y + 1, block.z);
        if target_y > y && !ai.tick.world.state(above).is_air() {
            state.height = state.height.min(-1.0);
            Self::select_next(state, ai);
        }
    }
}

/// `TargetingConditions.DEFAULT` as `Phantom.canAttack` asks it: a combat test, which no
/// creative player passes.
fn can_attack(ai: &mut Ai<'_, '_>, target: &sensing::Looked) -> bool {
    Conditions::combat().test(ai, target, &|_| true)
}

/// `PhantomAttackStrategyGoal`'s state: `nextSweepTick`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Striking {
    pub(crate) next_sweep_tick: i32,
}

/// `PhantomAttackStrategyGoal`: with a target it may attack, circling above it and every
/// 4 to 6 seconds swooping, with the swoop's sound.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhantomAttackStrategyGoal;

impl PhantomAttackStrategyGoal {
    /// `setAnchorAboveTarget()`: 20 to 39 above its target, not below the sea.
    fn set_anchor_above_target(ai: &mut Ai<'_, '_>) {
        if state_of(&ai.tick.mob).anchor.is_none() {
            return;
        }
        let Some(target) = sensing::target(ai) else {
            return;
        };
        let at = target.block_position();
        let mut anchor = BlockPos::new(at.x, at.y + 20 + ai.tick.draws.next_int(20) as i32, at.z);
        if anchor.y < SEA_LEVEL {
            anchor = BlockPos::new(anchor.x, SEA_LEVEL + 1, anchor.z);
        }
        change(&mut ai.tick.mob, |own| own.anchor = Some(anchor));
    }
}

impl Goal for PhantomAttackStrategyGoal {
    type State = Striking;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut Striking, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some_and(|target| can_attack(ai, &target))
    }

    fn start(&self, state: &mut Striking, ai: &mut Ai<'_, '_>) {
        state.next_sweep_tick = self.adjusted_tick_delay(10);
        change(&mut ai.tick.mob, |own| own.phase = AttackPhase::Circle);
        Self::set_anchor_above_target(ai);
    }

    /// Its anchor back to 10 to 29 above the ground under it.
    fn stop(&self, _: &mut Striking, ai: &mut Ai<'_, '_>) {
        let Some(anchor) = state_of(&ai.tick.mob).anchor else {
            return;
        };
        let ground = ai
            .tick
            .world
            .motion_blocking_height(anchor.x, anchor.z)
            .unwrap_or(anchor.y);
        let above = 10 + ai.tick.draws.next_int(20) as i32;
        change(&mut ai.tick.mob, |own| {
            own.anchor = Some(BlockPos::new(anchor.x, ground + above, anchor.z));
        });
    }

    fn tick(&self, state: &mut Striking, ai: &mut Ai<'_, '_>) {
        if state_of(&ai.tick.mob).phase != AttackPhase::Circle {
            return;
        }
        state.next_sweep_tick -= 1;
        if state.next_sweep_tick > 0 {
            return;
        }
        change(&mut ai.tick.mob, |own| own.phase = AttackPhase::Swoop);
        Self::set_anchor_above_target(ai);
        let delay = (8 + ai.tick.draws.next_int(4) as i32) * 20;
        state.next_sweep_tick = self.adjusted_tick_delay(delay);
        let pitch = 0.95 + ai.tick.draws.next_float() * 0.1;
        ai.tick.out.events.push(Event::Sound {
            sound: sounds::ENTITY_PHANTOM_SWOOP,
            volume: 10.0,
            pitch,
        });
    }
}

/// `PhantomSweepAttackGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Sweeping {
    pub(crate) cat_search_tick: i32,
    pub(crate) scared_of_cat: bool,
}

/// `PhantomSweepAttackGoal`: swooping at its target, hitting it where their boxes meet,
/// scared off by a cat within 16.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhantomSweepAttackGoal;

impl Goal for PhantomSweepAttackGoal {
    type State = Sweeping;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut Sweeping, ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.mob.body.combat.target.is_some()
            && state_of(&ai.tick.mob).phase == AttackPhase::Swoop
    }

    fn can_continue(&self, state: &mut Sweeping, ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = sensing::target(ai) else {
            return false;
        };
        if !target.alive || (target.is_player() && (target.spectator || target.creative)) {
            return false;
        }
        if !self.can_use(state, ai) {
            return false;
        }
        let tick_count = ai.tick_count() as i32;
        if tick_count > state.cat_search_tick {
            state.cat_search_tick = tick_count + 20;
            let bounds = ai_bounds(ai).inflate([16.0; 3]);
            let cat = entity_types::CAT as u16;
            state.scared_of_cat = ai
                .tick
                .world
                .entities_in(&bounds)
                .iter()
                .any(|seen| seen.entity.kind == cat && sensing::Looked::of_seen(seen).alive);
        }
        !state.scared_of_cat
    }

    fn stop(&self, _: &mut Sweeping, ai: &mut Ai<'_, '_>) {
        sensing::set_target(ai, None);
        change(&mut ai.tick.mob, |own| own.phase = AttackPhase::Circle);
    }

    fn tick(&self, _: &mut Sweeping, ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        let height = target.bounds.max[1] - target.bounds.min[1];
        let point = [
            target.position[0],
            target.position[1] + height * 0.5,
            target.position[2],
        ];
        change(&mut ai.tick.mob, |own| own.move_target = point);
        if ai_bounds(ai)
            .inflate([0.200_000_002_980_232_24; 3])
            .intersects(target.bounds)
        {
            let mob_type = ai.tick.mob_type;
            mob_type.do_hurt_target(ai, &target);
            change(&mut ai.tick.mob, |own| own.phase = AttackPhase::Circle);
            ai.tick.out.local.push(Local::LevelEvent {
                kind: 1039,
                pos: ai.block_position(),
                data: 0,
            });
        } else if ai.tick.living.body.horizontal_collision || hurt_recently(ai) {
            change(&mut ai.tick.mob, |own| own.phase = AttackPhase::Circle);
        }
    }
}

/// Its box now.
fn ai_bounds(ai: &Ai<'_, '_>) -> Aabb {
    crate::mobs::ai::goals::ai_bounds(ai)
}

/// `wasHurtRecently`, read as its hurt time running (10 moments from its hurt).
fn hurt_recently(ai: &Ai<'_, '_>) -> bool {
    ai.tick
        .mob
        .body
        .vitals
        .hurt_from
        .is_some_and(|from| i64::from(ai.tick.lived) - i64::from(from) < 10)
}

/// `PhantomAttackPlayerTargetGoal`'s state: `nextScanTick`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Scanning {
    pub(crate) next_scan_tick: i32,
}

/// `PhantomAttackPlayerTargetGoal`: every 30 of its goal ticks (`reducedTickDelay(60)`),
/// the highest player within 16 across and 64 up and down it may attack. No draws.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhantomAttackPlayerTargetGoal;

impl Goal for PhantomAttackPlayerTargetGoal {
    type State = Scanning;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Scanning, ai: &mut Ai<'_, '_>) -> bool {
        if state.next_scan_tick > 0 {
            state.next_scan_tick -= 1;
            return false;
        }
        state.next_scan_tick = reduced_tick_delay(60);
        let bounds = ai_bounds(ai).inflate([16.0, 64.0, 16.0]);
        // `attackTargeting`: a combat test within 64.
        let conditions = Conditions::combat().range(64.0);
        let mut players: Vec<sensing::Looked> = sensing::players(ai)
            .into_iter()
            .filter(|player| player.bounds.intersects(bounds))
            .filter(|player| conditions.test(ai, player, &|_| true))
            .collect();
        // Highest first (`Comparator.comparing(getY).reversed()`, a stable sort).
        players.sort_by(|a, b| b.position[1].total_cmp(&a.position[1]));
        for player in players {
            if can_attack(ai, &player) {
                sensing::set_target(ai, Some(&player));
                return true;
            }
        }
        false
    }

    fn can_continue(&self, _: &mut Scanning, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some_and(|target| can_attack(ai, &target))
    }
}

/// The phantom.
pub(crate) struct Phantom;

impl MobType for Phantom {
    fn register_goals(&self, setup: &mut Setup) {
        register_goals(setup);
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        tick_move_control(ai);
    }

    /// `PhantomLookControl.tick()`: nothing.
    fn tick_look_control(&self, _: &mut Ai<'_, '_>) {}

    /// Its own body control, run after the tick's `aiStep` (`after_tick`).
    fn body_rotation_ticks(&self, _: &MobTick<'_>) -> bool {
        false
    }

    /// `PhantomBodyRotationControl.clientTick()`: its head to its body, its body to its
    /// yaw.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        let head = &mut tick.mob.body.head;
        head.head_yaw = head.body_yaw;
        head.body_yaw = tick.yaw;
    }

    fn travel(&self, _: &MobTick<'_>) -> Travel {
        Travel::Flying {
            water: 0.02,
            lava: 0.02,
            air: 0.2,
        }
    }

    fn no_gravity(&self, _: &Mob) -> bool {
        true
    }

    fn on_climbable(&self, _: &Mob) -> Option<bool> {
        Some(false)
    }

    /// `Phantom.finalizeSpawn` before `Mob`'s: its anchor five above its block, its size
    /// 0.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let at = spawn.at;
        change(spawn.mob, |own| {
            own.anchor = Some(BlockPos::new(at.x, at.y + 5, at.z));
        });
        set_size(spawn.mob, 0);
    }

    /// `getDefaultDimensions`: its type's scaled by `1 + 0.15 · size`.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let info = EntityTypeInfo::of(entity_types::PHANTOM);
        let scale = 1.0 + 0.15 * state_of(mob).size as f32;
        Some(Dimensions {
            width: info.width * scale,
            height: info.height * scale,
            eye_height: info.eye_height * scale,
        })
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PHANTOM_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_PHANTOM_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PHANTOM_DEATH)
    }

    /// `xpReward` 5, with what its equipment adds.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(5, mob, draws)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let size = state_of(mob).size;
        vec![(SIZE_INDEX, DataShown::Int(size), size == 0)]
    }
}
