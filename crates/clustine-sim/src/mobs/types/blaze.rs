//! The blaze (`Blaze`, `Blaze$BlazeAttackGoal`, d javap; `docs/groundwork/p-brief.md`,
//! section 5.9; family E3 of P12, `docs/groundwork/p-plan.md`, sections 18.3, 18.5 and
//! 18.6): made by natural spawning where a Nether fortress's list names it, and by its
//! egg.
//!
//! **It hovers** (Blaze.txt 158–314): off the ground and falling, its fall is six tenths
//! at the head of each `aiStep`; in its `customServerAiStep` it draws a new height it
//! keeps above its target every 100 ticks (`triangle(0.5, 6.891)`), and while its
//! target's eyes are higher than its own by more than that, it climbs: its vertical
//! motion goes three tenths of the way to 0.3, sent at once (`needsSync`). Otherwise it
//! walks and falls as any monster, on a ground navigation that shuns water (−1) and
//! dislikes lava (8) and minds no fire (0).
//!
//! **It fights** (`BlazeAttackGoal`, 69–318): within 2 of its target and seeing it, a
//! hit every 20 ticks (`doHurtTarget`, its attack 6, no swing) and a move onto it; further
//! but within its follow range (48) and in sight, a volley: it charges for 60 ticks
//! (showing itself alight, `DATA_FLAGS_ID`'s bit 1), then shoots three small fireballs 6
//! ticks apart, each with level event 1018 and aimed at the target's middle with a
//! spread of `triangle(d, 2.297 × sqrt(sqrt(d²)) × 0.5)` across, from its middle half a
//! block up, then rests 100 ticks unlit; it looks at its target meanwhile. Out of sight
//! for fewer than 5 ticks it moves onto the target. Its targets: who hurt it, alerting
//! the blazes about it, and the nearest player it sees.
//!
//! **Its body**: immune to fire (its type's), hurt by water (`isSensitiveToWater`:
//! drowning's 1 a tick in water, after `LivingEntity.aiStep`'s move); `isOnFire` is its
//! charge; its light-dependent value is always 1 (`getLightLevelDependentMagicValue`),
//! so its idle time counts twice more everywhere. Its ambient, hurt and death sounds; 10
//! experience; its loot is its table's (a blaze rod where a player killed it).
//!
//! Named differences:
//! - Its burning sound and its smoke are the client's (`isClientSide` in `aiStep`): the
//!   server sends nothing of them.
//! - Rain never falls in Clustine (and never in the Nether), so only water hurts it.
//! - Its `isOnFire` read elsewhere (a loot condition, the fire's extinguishing sound) is
//!   its burning, as every entity's; no table or rule of a blaze's reads it.
//! - The fields the game keeps in memory (the height offset and its timer, the goal's
//!   steps) are its record's, so a blaze loaded again goes on where it was.

use clustine_data::{PathType, attributes, entity_types, sounds};
use clustine_world::TypeState;
use serde::{Deserialize, Serialize};

use super::MobType;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    LookAtPlayerGoal, MoveTowardsRestrictionGoal, RandomLookAroundGoal, RandomStrollGoal, Stroll,
};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Looked};
use crate::mobs::ai::targets::{self, NearestTarget, Target};
use crate::mobs::ai::{Ai, Setup, controls};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::{self, thrown::triangle};
use crate::region::Random;
use crate::shown::DataShown;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The version of [`BlazeState`]'s encoding.
const VERSION: u8 = 1;

/// `DATA_FLAGS_ID`'s index, the first after `Mob`'s; its bit 1 is the charge.
pub(crate) const FLAGS_INDEX: u8 = 16;

/// `LevelEvent.SOUND_BLAZE_FIREBALL`, at each of a volley's shots.
pub(crate) const SHOOT_EVENT: i32 = 1018;

/// What a blaze keeps: `allowedHeightOffset`, `nextHeightOffsetChangeTick` and the
/// synced charge (`DATA_FLAGS_ID`'s bit 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BlazeState {
    pub(crate) allowed_height_offset: f32,
    pub(crate) next_height_offset_change_tick: i32,
    pub(crate) charged: bool,
}

impl Default for BlazeState {
    /// The constructor's: an offset of 0.5 (Blaze.txt 6–9), the timer at 0, unlit.
    fn default() -> Self {
        Self {
            allowed_height_offset: 0.5,
            next_height_offset_change_tick: 0,
            charged: false,
        }
    }
}

/// A blaze's state from its type state; nothing, or what this build cannot read, as a
/// fresh blaze's.
pub(crate) fn state_of(mob: &Mob) -> BlazeState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return BlazeState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// The state changed by `change`, written back where it changed.
pub(crate) fn change(mob: &mut Mob, change: impl FnOnce(&mut BlazeState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = if own == BlazeState::default() {
            TypeState::default()
        } else {
            TypeState {
                version: VERSION,
                bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
            }
        };
    }
}

/// `Blaze.registerGoals`' targets (Blaze.txt 90–136): who hurt it, alerting the blazes
/// about it (`setAlertOthers()`, ignoring none); the nearest player it sees.
const TARGETS: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: &[],
            alert: true,
        },
    ),
    (2, Target::Nearest(NearestTarget::players(true))),
];

/// `Entity.getY(0.5)` of a thing of box `bounds` at `position`: its feet and half its
/// height (the float height, widened).
fn middle_of(target: &Looked) -> f64 {
    let height = (target.bounds.max[1] - target.bounds.min[1]) as f32;
    target.position[1] + f64::from(height) * 0.5
}

/// `BlazeAttackGoal`'s fields: `attackStep`, `attackTime`, `lastSeen`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Volley {
    pub(crate) attack_step: i32,
    pub(crate) attack_time: i32,
    pub(crate) last_seen: i32,
}

/// `Blaze$BlazeAttackGoal(blaze)`: of the move and look flags, every tick, while its
/// target is alive and one it can attack.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlazeAttackGoal;

impl BlazeAttackGoal {
    /// The shot of a volley's step (`tick` 286–461): the level event at its block, then
    /// one small fireball from its middle half a block up (`setPos(x, getY(0.5) + 0.5,
    /// z)`), along `(triangle(dx, 2.297 s), dy, triangle(dz, 2.297 s))` normalised, `s`
    /// the spread of the distance's fourth root halved; the triangles drawn of its own
    /// random, x first.
    fn shoot(ai: &mut Ai<'_, '_>, target: &Looked, distance_sqr: f64) {
        let [x, y, z] = ai.position();
        let middle = y + f64::from(ai.height()) * 0.5;
        let dx = target.position[0] - x;
        let dy = middle_of(target) - middle;
        let dz = target.position[2] - z;
        let spread = distance_sqr.sqrt().sqrt() * 0.5;
        ai.tick.out.local.push(Local::LevelEvent {
            kind: SHOOT_EVENT,
            pos: ai.block_position(),
            data: 0,
        });
        let across = triangle(ai.tick.draws, dx, 2.297 * spread);
        let along = triangle(ai.tick.draws, dz, 2.297 * spread);
        ai.tick.out.local.push(Local::ShootFireball {
            kind: entity_types::SMALL_FIREBALL as u16,
            at: [x, middle + 0.5, z],
            direction: physics::normalize([across, dy, along]),
            explosion_power: 0,
        });
    }
}

/// `setCharged(charged)`.
fn set_charged(mob: &mut Mob, charged: bool) {
    change(mob, |own| own.charged = charged);
}

impl Goal for BlazeAttackGoal {
    type State = Volley;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    /// `canUse()`: a target, alive, that it can attack.
    fn can_use(&self, _: &mut Volley, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some_and(|target| target.alive && sensing::can_attack(&target))
    }

    fn start(&self, state: &mut Volley, _: &mut Ai<'_, '_>) {
        state.attack_step = 0;
    }

    fn stop(&self, state: &mut Volley, ai: &mut Ai<'_, '_>) {
        set_charged(&mut ai.tick.mob, false);
        state.last_seen = 0;
    }

    fn tick(&self, state: &mut Volley, ai: &mut Ai<'_, '_>) {
        state.attack_time -= 1;
        let Some(target) = sensing::target(ai) else {
            return;
        };
        let sees = sensing::has_line_of_sight(ai, &target);
        if sees {
            state.last_seen = 0;
        } else {
            state.last_seen += 1;
        }
        let distance = target.distance_sqr(ai.position());
        let follow = ai.attribute(attributes::FOLLOW_RANGE);
        if distance < 4.0 {
            if !sees {
                return;
            }
            if state.attack_time <= 0 {
                state.attack_time = 20;
                let mob_type = ai.tick.mob_type;
                mob_type.do_hurt_target(ai, &target);
            }
            controls::set_wanted_position(ai, target.position, 1.0);
        } else if distance < follow * follow && sees {
            if state.attack_time <= 0 {
                state.attack_step += 1;
                if state.attack_step == 1 {
                    state.attack_time = 60;
                    set_charged(&mut ai.tick.mob, true);
                } else if state.attack_step <= 4 {
                    state.attack_time = 6;
                } else {
                    state.attack_time = 100;
                    state.attack_step = 0;
                    set_charged(&mut ai.tick.mob, false);
                }
                if state.attack_step > 1 {
                    Self::shoot(ai, &target, distance);
                }
            }
            controls::set_look_at(
                ai,
                [target.position[0], target.eye_y, target.position[2]],
                10.0,
                10.0,
            );
        } else if state.last_seen < 5 {
            controls::set_wanted_position(ai, target.position, 1.0);
        }
    }
}

/// `getTarget()` as `customServerAiStep` reads it, where its tick has no AI: the thing
/// its target names, wherever its view has it, if still a valid target.
fn target_of(tick: &MobTick<'_>) -> Option<Looked> {
    let key = tick.mob.body.combat.target?;
    let found = match tick.world.entity(key) {
        Some(seen) => Some(Looked::of_seen(&seen)),
        None => tick
            .world
            .players()
            .iter()
            .find(|player| player.key == key)
            .map(Looked::of_player),
    };
    found.filter(sensing::valid_target)
}

/// `Blaze.customServerAiStep` (Blaze.txt 251–318), before `Monster`'s: its height
/// offset drawn again every 100 ticks, and its climb towards a target whose eyes are
/// above its own by more than it.
pub(crate) fn custom_server_ai_step(tick: &mut MobTick<'_>) {
    let mut own = state_of(&tick.mob);
    own.next_height_offset_change_tick -= 1;
    if own.next_height_offset_change_tick <= 0 {
        own.next_height_offset_change_tick = 100;
        own.allowed_height_offset = triangle(tick.draws, 0.5, 6.891) as f32;
    }
    let offset = own.allowed_height_offset;
    change(&mut tick.mob, |kept| *kept = own);
    let Some(target) = target_of(tick) else {
        return;
    };
    let eyes = tick.living.body.position[1] + f64::from(tick.living.eye_height);
    if target.eye_y > eyes + f64::from(offset) && sensing::can_attack(&target) {
        let motion = &mut tick.living.body.motion;
        motion[1] += (f64::from(0.3_f32) - motion[1]) * f64::from(0.3_f32);
        tick.out.needs_sync = true;
    }
}

/// The blaze.
pub(crate) struct Blaze;

impl MobType for Blaze {
    /// `Blaze.registerGoals` (Blaze.txt 39–109) and its constructor's maluses (6–37).
    fn register_goals(&self, setup: &mut Setup) {
        setup.set_malus(PathType::Water, -1.0);
        setup.set_malus(PathType::Lava, 8.0);
        setup.set_malus(PathType::FireInNeighbor, 0.0);
        setup.set_malus(PathType::Fire, 0.0);
        setup.add_goal(4, BlazeAttackGoal);
        setup.add_goal(5, MoveTowardsRestrictionGoal { speed: 1.0 });
        // `WaterAvoidingRandomStrollGoal(this, 1.0, 0.0f)`: always to land.
        setup.add_goal(
            7,
            RandomStrollGoal {
                kind: Stroll::AvoidingWater { probability: 0.0 },
                ..RandomStrollGoal::new(1.0)
            },
        );
        setup.add_goal(8, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(8, RandomLookAroundGoal);
        targets::register(setup, TARGETS);
    }

    /// `Blaze.aiStep`'s head (158–176): off the ground and falling, six tenths of the
    /// fall. And, for `Monster.updateNoActionTime` just after, which reads the type's
    /// light-dependent value (always 1 for a blaze) where Clustine reads the light at
    /// its eyes: the two more idle ticks the light did not give it.
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        let body = &mut tick.living.body;
        if !body.on_ground && body.motion[1] < 0.0 {
            body.motion[1] *= 0.6;
        }
        if tick.mob.body.idle.no_action_since.is_some()
            && crate::mobs::body::burning::magic_value(tick) <= 0.5
        {
            let gained = &mut tick.mob.body.idle_gained.gained;
            *gained = gained.saturating_add(2);
        }
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        custom_server_ai_step(tick);
    }

    /// `LivingEntity.aiStep`'s water hurt (`isSensitiveToWater`, Blaze.txt 246–249): in
    /// water, drowning's 1.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        if tick.living.fluids.in_water() && !tick.mob.body.vitals.dying() {
            crate::mobs::body::hurt_by(tick, &DamageSource::of(DamageType::DROWN), 1.0);
        }
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_BLAZE_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_BLAZE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_BLAZE_DEATH)
    }

    /// `xpReward` 10 (the constructor's), with what its equipment adds.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(10, mob, draws)
    }

    /// `DATA_FLAGS_ID`: its charge as bit 1.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let charged = state_of(mob).charged;
        vec![(FLAGS_INDEX, DataShown::Byte(u8::from(charged)), !charged)]
    }
}

#[cfg(test)]
#[path = "nether_e3_tests.rs"]
mod tests;
