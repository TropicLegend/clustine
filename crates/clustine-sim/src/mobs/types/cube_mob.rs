//! What the slime's kind share (`AbstractCubeMob` and its inner goals and move control;
//! `docs/groundwork/n9-plan.md`, section 8.4; n9 AbstractCubeMob): its size, its jumps,
//! its four goals, its landing and its split at death.
//!
//! A cube is an `AgeableMob` that is never a baby (the slime's `canBeABaby` is false):
//! its age is always 0, and its synced baby and age lock (16 and 17) never change. Its
//! size (`ID_SIZE`, data 18) is 1 to 127, health `n²`, speed `0.2 + 0.1n`, and it is
//! `n` times its type's box. It moves by jumping: its move control turns it to the way a
//! goal set, and on the ground counts down a delay (`10 + nextInt(20)`, a third of that
//! when it means harm) between jumps, with the jump's sound.
//!
//! Its move control's own fields (the way it wants to face, its jump delay, whether it
//! means harm) are kept in its type state, where the game keeps them in memory and makes
//! them afresh at a load (named: a cube loaded again keeps counting its delay).
//!
//! Named differences: the move control's first way is the mob's yaw as made, where the
//! game's constructor takes `180 · yRot / π` of its constructor's yaw (degrees read as
//! radians), which the random direction goal replaces at its first tick on the ground;
//! `jumpFromGround` keeps an upward motion above its jump's (`LivingEntity`'s), where the
//! cube's sets the jump's whatever it was; a cube hurts no iron golem that pushes it
//! (`push`'s `dealDamage`), as the region does not tell a mob what it is pushed by.

use clustine_data::{EntityTypeInfo, attributes as ids};
use clustine_world::TypeState;
use serde::{Deserialize, Serialize};

use super::{Dimensions, SpawnGroup, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::controls;
use crate::mobs::ai::maths::{self, move_rotlerp, reduced_tick_delay};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing;
use crate::mobs::ai::{Ai, HeadLimits, Operation};
use crate::mobs::body::Event;
use crate::mobs::body::attribute;
use crate::mobs::body::effects::Effect;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// The version of a cube's type state.
const VERSION: u8 = 1;

/// `AbstractCubeMob.MAX_SIZE`.
pub(crate) const MAX_SIZE: i32 = 127;

/// `ID_SIZE`'s index, after `AgeableMob`'s baby (16) and age lock (17).
const SIZE_INDEX: u8 = 18;
const BABY_INDEX: u8 = 16;
const AGE_LOCKED_INDEX: u8 = 17;

/// What a cube keeps: its size less one (as the game saves `Size`, so that a record of
/// nothing is a cube of size 1), `wasOnGround`, and its move control's fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CubeState {
    pub(crate) size_less_one: i32,
    pub(crate) was_on_ground: bool,
    /// `CubeMobMoveControl.yRot`: the way a goal last set, none before any did.
    pub(crate) direction: Option<f32>,
    /// `CubeMobMoveControl.jumpDelay`.
    pub(crate) jump_delay: i32,
    /// `CubeMobMoveControl.isAggressive`.
    pub(crate) aggressive: bool,
}

/// A cube's state from its type state; a record of nothing, or one this build cannot
/// read, as a fresh cube's of size 1.
pub(crate) fn state_of(mob: &Mob) -> CubeState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return CubeState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// The cube's state changed by `change`, written back where it changed.
pub(crate) fn change(mob: &mut Mob, change: impl FnOnce(&mut CubeState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = if own == CubeState::default() {
            TypeState::default()
        } else {
            TypeState {
                version: VERSION,
                bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
            }
        };
    }
}

/// `getSize()`.
pub(crate) fn size(mob: &Mob) -> i32 {
    state_of(mob).size_less_one.saturating_add(1)
}

/// `isTiny()`: of size 1 (or less).
pub(crate) fn is_tiny(mob: &Mob) -> bool {
    size(mob) <= 1
}

/// `Slime.setSize(size, resetHealth)` of a cube of the type `kind`: the size held to 1 to
/// 127, its maximum health `n²`, its speed `0.2 + 0.1n` and its attack `n` as their bases,
/// and where `reset_health`, its health its maximum. (The size's change turns its yaw and
/// body to its head, which every maker of a cube has already done.)
pub(crate) fn set_size(kind: u16, mob: &mut Mob, size: i32, reset_health: bool) {
    let size = size.clamp(1, MAX_SIZE);
    change(mob, |own| own.size_less_one = size - 1);
    let attributes = &mut mob.body.attributes;
    if let Some(health) = attributes.instance(kind, attribute(ids::MAX_HEALTH)) {
        health.base = f64::from(size * size);
    }
    if let Some(speed) = attributes.instance(kind, attribute(ids::MOVEMENT_SPEED)) {
        speed.base = f64::from(0.2_f32 + 0.1_f32 * size as f32);
    }
    if reset_health {
        mob.body.vitals.health = attributes.value(kind, attribute(ids::MAX_HEALTH)) as f32;
    }
    if let Some(attack) = attributes.instance(kind, attribute(ids::ATTACK_DAMAGE)) {
        attack.base = f64::from(size);
    }
}

/// `getDefaultDimensions(pose)`: its type's scaled by its size.
pub(crate) fn dimensions(kind: u16, mob: &Mob) -> Dimensions {
    let info = EntityTypeInfo::of(i32::from(kind));
    let scale = size(mob) as f32;
    Dimensions {
        width: info.width * scale,
        height: info.height * scale,
        eye_height: info.eye_height * scale,
    }
}

/// `getSoundVolume()`: `0.4 · n`.
pub(crate) fn sound_volume(mob: &Mob) -> f32 {
    0.4 * size(mob) as f32
}

/// `getSoundPitch()`: about 1.4 for a tiny cube, 0.8 for a larger, two floats of its own.
fn sound_pitch(mob: &Mob, draws: &mut Random) -> f32 {
    let base = if is_tiny(mob) { 1.4 } else { 0.8 };
    let a = draws.next_float();
    let b = draws.next_float();
    ((a - b) * 0.2 + 1.0) * base
}

/// `getMaxHeadXRot()`: 0, its head never tilts.
pub(crate) fn head() -> HeadLimits {
    HeadLimits {
        max_x_rot: 0,
        ..HeadLimits::default()
    }
}

/// `AbstractCubeMob.registerGoals`: the float (1), the random way (4) and the keeping on
/// jumping (5), then its kind's behaviour (`addBehaviourGoals`, the slime's attack at 2).
pub(crate) fn register_goals(setup: &mut crate::mobs::ai::Setup) {
    setup.add_goal(1, CubeMobFloatGoal);
    setup.add_goal(4, CubeMobRandomDirectionGoal);
    setup.add_goal(5, CubeMobKeepOnJumpingGoal);
}

/// `CubeMobMoveControl.setDirection(yRot, aggressive)`.
fn set_direction(mob: &mut Mob, yaw: f32, aggressive: bool) {
    change(mob, |own| {
        own.direction = Some(yaw);
        own.aggressive = aggressive;
    });
}

/// `CubeMobMoveControl.setWantedMovement(speed)`: on, at `speed` times its own.
fn set_wanted_movement(ai: &mut Ai<'_, '_>, speed: f64) {
    let movement = &mut ai.tick.mob.body.controls.movement;
    movement.speed = speed;
    movement.operation = Operation::MoveTo;
}

/// `Mob.setSpeed(speed)`, which sets `zza` too.
fn set_speed(ai: &mut Ai<'_, '_>, speed: f32) {
    let inputs = &mut ai.tick.mob.body.inputs;
    inputs.speed = speed;
    inputs.zza = speed;
}

/// `canDealDamage()`: not tiny, and its AI its own.
fn can_deal_damage(ai: &Ai<'_, '_>) -> bool {
    !is_tiny(&ai.tick.mob) && ai.tick.effective_ai()
}

/// `CubeMobMoveControl.tick()`: turned towards its way by at most 90 (head and body with
/// it); and where a goal asked it on, on the ground its speed set and its jump's delay
/// counted down, a jump made with its sound once it is over, and off the ground its speed
/// set alone.
pub(crate) fn tick_move_control(ai: &mut Ai<'_, '_>) {
    let own = state_of(&ai.tick.mob);
    let wanted = own.direction.unwrap_or(ai.tick.yaw);
    let yaw = move_rotlerp(ai.tick.yaw, wanted, 90.0);
    ai.tick.yaw = yaw;
    ai.tick.mob.body.head.head_yaw = yaw;
    ai.tick.mob.body.head.body_yaw = yaw;
    let movement = ai.tick.mob.body.controls.movement.clone();
    if movement.operation != Operation::MoveTo {
        ai.tick.mob.body.inputs.zza = 0.0;
        return;
    }
    ai.tick.mob.body.controls.movement.operation = Operation::Wait;
    let speed = (movement.speed * ai.attribute(ids::MOVEMENT_SPEED)) as f32;
    if !ai.on_ground() {
        set_speed(ai, speed);
        return;
    }
    set_speed(ai, speed);
    let was = own.jump_delay;
    if was > 0 {
        change(&mut ai.tick.mob, |own| own.jump_delay = was - 1);
        let inputs = &mut ai.tick.mob.body.inputs;
        inputs.xxa = 0.0;
        inputs.zza = 0.0;
        set_speed(ai, 0.0);
        return;
    }
    // `getJumpDelay()`: `10 + nextInt(20)`, a third of that when it means harm.
    let mut delay = ai.tick.draws.next_int(20) as i32 + 10;
    if own.aggressive {
        delay /= 3;
    }
    change(&mut ai.tick.mob, |own| own.jump_delay = delay);
    controls::jump(ai);
    // `doPlayJumpSound()`: of any size above 0, every cube's.
    let sound = sounds(ai.tick.kind).of(&ai.tick.mob).jump;
    let volume = sound_volume(&ai.tick.mob);
    let pitch = sound_pitch(&ai.tick.mob, ai.tick.draws);
    ai.tick.out.events.push(Event::Sound {
        sound,
        volume,
        pitch,
    });
}

/// `Mob.lookAt(entity, maxYaw, maxPitch)` of the target's eyes: its yaw and pitch turned
/// towards them by at most so much (`Mob.rotlerp`, unwrapped).
fn look_at(ai: &mut Ai<'_, '_>, target: &sensing::Looked, max_yaw: f32, max_pitch: f32) {
    let [x, _, z] = ai.position();
    let dx = target.position[0] - x;
    let dz = target.position[2] - z;
    let dy = target.eye_y - ai.eye_y();
    let horizontal = (dx * dx + dz * dz).sqrt();
    let yaw = maths::yaw_towards(dx, dz);
    let pitch = maths::pitch_towards(dy, horizontal);
    let turn =
        |from: f32, to: f32, max: f32| from + maths::wrap_degrees(to - from).clamp(-max, max);
    ai.tick.pitch = turn(ai.tick.pitch, pitch, max_pitch);
    ai.tick.yaw = turn(ai.tick.yaw, yaw, max_yaw);
}

/// `CubeMobAttackGoal`'s state: `growTiredTimer`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tiring {
    pub(crate) grow_tired_timer: i32,
}

/// `CubeMobAttackGoal`: at its target for 150 of its goal ticks (`reducedTickDelay(300)`),
/// turned to it at 10 a tick, jumping its way, meaning harm unless it is tiny.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CubeMobAttackGoal;

impl Goal for CubeMobAttackGoal {
    type State = Tiring;

    fn flags(&self) -> u8 {
        flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut Tiring, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some_and(|target| sensing::can_attack(&target))
    }

    fn can_continue(&self, state: &mut Tiring, ai: &mut Ai<'_, '_>) -> bool {
        if !sensing::target(ai).is_some_and(|target| sensing::can_attack(&target)) {
            return false;
        }
        state.grow_tired_timer -= 1;
        state.grow_tired_timer > 0
    }

    fn start(&self, state: &mut Tiring, _: &mut Ai<'_, '_>) {
        state.grow_tired_timer = reduced_tick_delay(300);
    }

    fn tick(&self, _: &mut Tiring, ai: &mut Ai<'_, '_>) {
        if let Some(target) = sensing::target(ai) {
            look_at(ai, &target, 10.0, 10.0);
        }
        let yaw = ai.tick.yaw;
        let aggressive = can_deal_damage(ai);
        set_direction(&mut ai.tick.mob, yaw, aggressive);
    }
}

/// `CubeMobFloatGoal`: in water or lava, a jump four times in five and on at 1.2. Building
/// it lets the navigation float.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CubeMobFloatGoal;

impl Goal for CubeMobFloatGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::JUMP | flags::MOVE
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn on_register(&self, config: &mut crate::mobs::ai::Config) {
        config.navigation.set_can_float(true);
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        ai.in_liquid()
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        if ai.tick.draws.next_float() < 0.8 {
            controls::jump(ai);
        }
        set_wanted_movement(ai, 1.2);
    }
}

/// `CubeMobRandomDirectionGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Wandering {
    pub(crate) chosen_degrees: f32,
    pub(crate) next_randomize_time: i32,
}

/// `CubeMobRandomDirectionGoal`: with no target, on the ground, in a liquid or levitating,
/// a new way every 20 to 49 of its goal ticks (`adjustedTickDelay(40 + nextInt(60))`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct CubeMobRandomDirectionGoal;

impl Goal for CubeMobRandomDirectionGoal {
    type State = Wandering;

    fn flags(&self) -> u8 {
        flags::LOOK
    }

    fn can_use(&self, _: &mut Wandering, ai: &mut Ai<'_, '_>) -> bool {
        let levitating = Effect::named("minecraft:levitation")
            .is_some_and(|levitation| ai.tick.mob.body.effects.has(levitation));
        ai.tick.mob.body.combat.target.is_none() && (ai.on_ground() || ai.in_liquid() || levitating)
    }

    fn tick(&self, state: &mut Wandering, ai: &mut Ai<'_, '_>) {
        state.next_randomize_time -= 1;
        if state.next_randomize_time <= 0 {
            let delay = 40 + ai.tick.draws.next_int(60) as i32;
            state.next_randomize_time = self.adjusted_tick_delay(delay);
            state.chosen_degrees = ai.tick.draws.next_int(360) as f32;
        }
        set_direction(&mut ai.tick.mob, state.chosen_degrees, false);
    }
}

/// `CubeMobKeepOnJumpingGoal`: riding nothing, always on at its own speed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CubeMobKeepOnJumpingGoal;

impl Goal for CubeMobKeepOnJumpingGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::JUMP | flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.vehicle.is_none()
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        set_wanted_movement(ai, 1.0);
    }
}

/// `AbstractCubeMob.tick()` after its parent's: where it has just landed, the particles'
/// draws (two floats each, `width · 32` of them, which a server shows nothing of) and its
/// squish at `0.4 · n`; `wasOnGround` kept.
pub(crate) fn after_tick(tick: &mut MobTick<'_>) {
    let on_ground = tick.living.body.on_ground;
    let own = state_of(&tick.mob);
    if on_ground && !own.was_on_ground {
        let width = dimensions(tick.kind, &tick.mob).width * 2.0;
        let mut drawn = 0;
        while (drawn as f32) < width * 16.0 {
            let _angle = tick.draws.next_float() * std::f32::consts::TAU;
            let _spread = tick.draws.next_float() * 0.5 + 0.5;
            drawn += 1;
        }
        let sound = sounds(tick.kind).of(&tick.mob).squish;
        let volume = sound_volume(&tick.mob);
        let a = tick.draws.next_float();
        let b = tick.draws.next_float();
        let pitch = ((a - b) * 0.2 + 1.0) / 0.8;
        tick.out.events.push(Event::Sound {
            sound,
            volume,
            pitch,
        });
    }
    if own.was_on_ground != on_ground {
        change(&mut tick.mob, |own| own.was_on_ground = on_ground);
    }
}

/// `AbstractCubeMob.finalizeSpawn` with the slime's group: `AgeableMobGroupData(false)`
/// where none is given, of which no member is a baby and nothing is drawn; then counted.
pub(crate) fn finalize_spawn(spawn: &mut Spawning<'_>) {
    let group = spawn
        .group
        .get_or_insert(SpawnGroup::Ageable(super::AgeableGroup::new(false)));
    let ageable = group.ageable();
    // `AgeableMob.finalizeSpawn`: a baby only of a group that makes them, which this
    // one does not, so nothing is drawn.
    if ageable.babies && ageable.made > 0 {
        let _baby = spawn.level.next_float() <= ageable.baby_chance;
    }
    ageable.made += 1;
}

/// `setSpawnSize(level, difficulty)`, after `Mob.finalizeSpawn`: of the level's random,
/// `nextInt(3)`, one more where that is under 2 and a float falls under half the special
/// multiplier; the size `1 << that`, its health reset.
pub(crate) fn set_spawn_size(kind: u16, spawn: &mut Spawning<'_>) {
    let mut power = spawn.level.next_int(3);
    if power < 2 && spawn.level.next_float() < 0.5 * spawn.difficulty.1 {
        power += 1;
    }
    set_size(kind, spawn.mob, 1 << power, true);
}

/// `getBaseExperienceReward()`: its `xpReward`, which is its size, with what its
/// equipment adds.
pub(crate) fn experience_reward(mob: &Mob, draws: &mut Random) -> i32 {
    super::illagers::experience(size(mob), mob, draws)
}

/// The synced values: `AgeableMob`'s baby and age lock, never set, and the size.
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let size = size(mob);
    vec![
        (BABY_INDEX, DataShown::Boolean(false), true),
        (AGE_LOCKED_INDEX, DataShown::Boolean(false), true),
        (SIZE_INDEX, DataShown::Int(size), size == 1),
    ]
}

/// One cube made of a larger one's split (`remove(reason)` of a dying cube): its size,
/// where it is made beside its parent (its offsets in x and z), and its yaw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Split {
    pub(crate) size: i32,
    pub(crate) offset: [f32; 2],
    pub(crate) yaw: f32,
}

/// `AbstractCubeMob.remove(KILLED)` of a dying cube of the type `kind`, of the parent's own
/// stream as its tick left it: `2 + nextInt(3)` cubes of half its size, each set out at
/// half its width times its place (`i % 2 − 0.5` across, `i / 2 − 0.5` along) and, once
/// made, turned to `nextFloat() · 360` (`setUpSplitCube`). Each new cube's own draws are
/// of its own stream, so the parent's are these alone, in this order. None for a tiny
/// cube.
pub(crate) fn split(kind: u16, parent: &Mob, draws: &mut Random) -> Vec<Split> {
    let size = size(parent);
    if size <= 1 {
        return Vec::new();
    }
    let half_width = dimensions(kind, parent).width / 2.0;
    let half = size / 2;
    let count = 2 + draws.next_int(3) as i32;
    (0..count)
        .map(|i| Split {
            size: half,
            offset: [
                ((i % 2) as f32 - 0.5) * half_width,
                ((i / 2) as f32 - 0.5) * half_width,
            ],
            yaw: draws.next_float() * 360.0,
        })
        .collect()
}

/// A kind of cube's sounds: those of a tiny one, and of a larger.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CubeSounds {
    pub(crate) small: Sounds,
    pub(crate) large: Sounds,
}

/// A cube's sounds of one size.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sounds {
    pub(crate) jump: i32,
    pub(crate) squish: i32,
    pub(crate) hurt: i32,
    pub(crate) death: i32,
}

impl CubeSounds {
    /// Those of `mob` by its size (`isTiny`).
    pub(crate) fn of(self, mob: &Mob) -> Sounds {
        if is_tiny(mob) { self.small } else { self.large }
    }
}

/// The sounds of the cube type `kind`: the slime's, the only cube built (the sulfur cube
/// is out of N9, and the magma cube of the Nether).
fn sounds(kind: u16) -> CubeSounds {
    let _ = kind;
    super::slime::SOUNDS
}
