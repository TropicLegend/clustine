//! The ravager (`Ravager`, n7 Ravager.txt; `docs/groundwork/n10-plan.md`, decision 10
//! and section 9.7; V 7.3): a raider, not an illager, of a hundred health that floats,
//! strolls away from water, looks about and chases what it takes as a target, players
//! it sees, grown villagers and iron golems, hitting for 12 and throwing what it hits
//! back. Each hit holds it still for ten ticks (`attackTick`); a hit a shield stops
//! stuns it one time in two for forty (`stunnedTick`), after which it roars
//! (`roarTick`): ten ticks on it hurts the living within 4 of its box but the
//! illagers and throws back all of them but players. In its way it breaks leaves, and
//! where it meets anything else it jumps. A raid rides a raider on it at the waves the
//! raid decides (R1's); a raider in its first seat steers it and leaves it its own
//! goals all the same (`updateControlFlags`).
//!
//! Its counters are the game's, counted down in its `aiStep` and saved (`AttackTick`,
//! `StunTick`, `RoarTick`), as the stun's and the roar's ticks are read by the server.
//!
//! Named differences: no entity blocks with a shield in Clustine (every player is
//! creative, and no mob holds one), so [`blocked_by_item`], the stun's beginning, has
//! no caller until one does; a player is neither hurt by the roar (creative) nor
//! pushed by the server (the game pushes them on their own client, by entity event
//! 69, which is sent); the roar pushes no mob in a copy (`Local::Push` reaches only
//! the region's own) and no armour stand (which the game pushes: its hurt by a mob's
//! attack the game refuses, `#can_break_armor_stand`); the stun's particles are the
//! client's, their draws the server's; `hasLineOfSight`'s refusal while stunned or
//! roaring is not ported, as the ravager is immobile then and its AI, the only reader,
//! does not run; a rider steering it is taken to be a raider, the only kind any rule
//! of Clustine seats on it; `checkSpawnObstruction` has no caller (no rule spawns a
//! ravager but a raid, an egg and a command, none of which asks it).

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, PathType, attributes as ids, entity_types, sounds};
use clustine_world::BlockPos;
use serde::{Deserialize, Serialize};

use super::illagers;
use super::{MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    Attacking, FloatGoal, LookAtPlayerGoal, MeleeAttackGoal, RandomStrollGoal, ai_bounds, hold,
    move_to_held,
};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Looked};
use crate::mobs::ai::targets::{self, NearestTarget, Target};
use crate::mobs::ai::{Ai, HeadLimits, Setup, navigation};
use crate::mobs::body::hurt::{DamageSource, GameEvent};
use crate::mobs::body::{self, Event, attribute};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::Vec3;
use crate::region::Random;
use crate::shown::DataShown;

#[cfg(test)]
#[path = "ravager_n10_tests.rs"]
mod tests;

/// How long a hit holds it still (`ATTACK_DURATION`).
pub(crate) const ATTACK_DURATION: i32 = 10;

/// How long a stun lasts (`STUN_DURATION`).
pub(crate) const STUN_DURATION: i32 = 40;

/// The roar's counter as a stun ends: it roars at `ROAR_AT`, ten ticks on.
pub(crate) const ROAR_DURATION: i32 = 20;

/// The roar's counter at which it roars.
pub(crate) const ROAR_AT: i32 = 10;

/// How far beyond its box the roar reaches.
const ROAR_REACH: f64 = 4.0;

/// What the roar hurts.
pub(crate) const ROAR_DAMAGE: f32 = 6.0;

/// Its speed with no target (`BASE_MOVEMENT_SPEED`) and with one
/// (`ATTACK_MOVEMENT_SPEED`), which its base speed is drawn a tenth of the way towards
/// each tick.
const BASE_SPEED: f64 = 0.3;
const ATTACK_SPEED: f64 = 0.35;

/// How much of its box beyond it the leaves it breaks may be.
const LEAVES_REACH: f64 = 0.2;

/// The sound of its celebration (`getCelebrateSound`), which a raid's celebration
/// plays (R1's).
#[allow(
    dead_code,
    reason = "a raid's celebration (R1) plays it; until then nothing does"
)]
pub(crate) const CELEBRATE_SOUND: i32 = sounds::ENTITY_RAVAGER_CELEBRATE;

/// The ravager.
pub(crate) struct Ravager;

/// The ravager's own part of its raider state (`crate::mobs::types::illagers`): its
/// three counters, as the game saves them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RavagerState {
    /// `attackTick` (`AttackTick`).
    pub(crate) attack_tick: i32,
    /// `stunnedTick` (`StunTick`).
    pub(crate) stunned_tick: i32,
    /// `roarTick` (`RoarTick`).
    pub(crate) roar_tick: i32,
}

/// The ravager's state of `mob`.
pub(crate) fn state_of(mob: &Mob) -> RavagerState {
    illagers::own_of(mob)
}

/// The `AbstractIllager` types, whom its roar does not hurt (the witch and the
/// ravager are raiders and no illagers).
fn is_illager(kind: u16) -> bool {
    [
        entity_types::PILLAGER,
        entity_types::VINDICATOR,
        entity_types::EVOKER,
        entity_types::ILLUSIONER,
    ]
    .contains(&i32::from(kind))
}

/// The villager goal's selector (`lambda$registerGoals$0`): no baby.
fn grown(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    targets::illager_can_attack(ai, looked)
}

/// `Ravager.registerGoals`' targets: who hurt it but a raider, alerting ravagers;
/// players it sees; grown villagers and iron golems it sees.
const TARGETS: &[(i32, Target)] = &[
    (
        2,
        Target::HurtBy {
            ignore: targets::RAIDERS,
            alert: true,
        },
    ),
    (3, Target::Nearest(NearestTarget::players(true))),
    (
        4,
        Target::Nearest(NearestTarget {
            selector: Some(grown),
            ..NearestTarget::mobs(targets::VILLAGERS, true)
        }),
    ),
    (
        4,
        Target::Nearest(NearestTarget::mobs(targets::IRON_GOLEMS, true)),
    ),
];

/// Its `MeleeAttackGoal(mob, 1.0, true)`, whose reach the ravager's own
/// `getAttackBoundingBox` narrows.
const MELEE: MeleeAttackGoal = MeleeAttackGoal {
    speed: 1.0,
    following_even_if_not_seen: true,
};

/// The ravager's `MeleeAttackGoal(1.0, true)`: the game's goal, its reach
/// (`isWithinMeleeAttackRange`) by the ravager's `getAttackBoundingBox`, `Raider`'s
/// less 0.05 in x and z.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RavagerAttackGoal;

impl RavagerAttackGoal {
    /// `isWithinMeleeAttackRange(target)`: its box grown by the default reach, then
    /// deflated by 0.05 across, meets the target's. It rides nothing it steers, so its
    /// box is its own.
    pub(crate) fn within_range(ai: &Ai<'_, '_>, target: &Looked) -> bool {
        let reach = f64::from(2.04_f32).sqrt() - f64::from(0.6_f32);
        ai_bounds(ai)
            .inflate([reach, 0.0, reach])
            .inflate([-0.05, 0.0, -0.05])
            .intersects(target.bounds)
    }
}

impl Goal for RavagerAttackGoal {
    type State = Attacking;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    /// `MeleeAttackGoal.canUse`, of its reach.
    fn can_use(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        let now = ai.now();
        if state.last_can_use_check.is_some_and(|last| now - last < 20) {
            return false;
        }
        state.last_can_use_check = Some(now);
        let Some(target) = sensing::target(ai) else {
            return false;
        };
        if !target.alive {
            return false;
        }
        let made = navigation::create_path(ai, target.block_position(), 0);
        state.path = hold(ai, made);
        if state.path.is_some() {
            return true;
        }
        Self::within_range(ai, &target)
    }

    fn can_continue(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        MELEE.can_continue(state, ai)
    }

    fn start(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        let held = state.path.take();
        move_to_held(ai, held, MELEE.speed);
        ai.tick.mob.body.combat.aggressive = true;
        state.recalc = 0;
        state.attack_cooldown = 0;
    }

    fn stop(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        MELEE.stop(state, ai);
    }

    /// `MeleeAttackGoal.tick`: the chase, then `checkAndPerformAttack` of its reach.
    fn tick(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        let Some(target) = MELEE.chase(state, ai) else {
            return;
        };
        if state.attack_cooldown <= 0
            && Self::within_range(ai, &target)
            && sensing::has_line_of_sight(ai, &target)
        {
            MELEE.reset_attack_cooldown(state);
            ai.tick.out.local.push(Local::Swing);
            let mob_type = ai.tick.mob_type;
            mob_type.do_hurt_target(ai, &target);
        }
    }
}

/// `strongKnockback(entity)` of an entity at `to` by a ravager at `from`: the push
/// `Entity.push(dx / d · 4, 0.2, dz / d · 4)`, `d` the square of their distance across
/// (at least 0.001), as the game divides by it.
pub(crate) fn strong_knockback(from: Vec3, to: Vec3) -> Vec3 {
    let dx = to[0] - from[0];
    let dz = to[2] - from[2];
    let d = (dx * dx + dz * dz).max(0.001);
    [dx / d * 4.0, 0.2, dz / d * 4.0]
}

/// What a ravager does as a shield stops its hit ([`blocked_by_item`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Blocked {
    /// It is stunned, and the blocker is pushed as one entity pushes another it meets
    /// (`blocker.push(ravager)`).
    Stunned,
    /// The blocker is thrown back by this push ([`strong_knockback`]).
    Knocked(Vec3),
}

/// `Ravager.blockedByItem(blocker, …)`: where it is not roaring, one draw of its own
/// stream: under a half stuns it for 40 ticks, its stunned sound and entity event 39;
/// else the blocker, at `blocker`, is thrown back from the ravager at `at`. Either way
/// the blocker's motion is sent (`syncVelocity`), the caller's to do; nothing where it
/// is roaring. **Named**: nothing blocks yet, so nothing calls it.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "no entity of Clustine blocks with a shield until survival comes"
    )
)]
pub(crate) fn blocked_by_item(
    mob: &mut Mob,
    own: &mut Random,
    at: Vec3,
    blocker: Vec3,
    events: &mut Vec<Event>,
) -> Option<Blocked> {
    if state_of(mob).roar_tick != 0 {
        return None;
    }
    if own.next_double() < 0.5 {
        illagers::update_own(mob, |own: &mut RavagerState| {
            own.stunned_tick = STUN_DURATION;
        });
        events.push(Event::Sound {
            sound: sounds::ENTITY_RAVAGER_STUNNED,
            volume: 1.0,
            pitch: 1.0,
        });
        events.push(Event::Entity(39));
        Some(Blocked::Stunned)
    } else {
        Some(Blocked::Knocked(strong_knockback(at, blocker)))
    }
}

/// `stunEffect()`: one time in six a particle beside its head, at two draws of its
/// own; the particle is the client's, the draws are the server's too.
fn stun_effect(tick: &mut MobTick<'_>) {
    if tick.draws.next_int(6) == 0 {
        tick.draws.next_double();
        tick.draws.next_double();
    }
}

/// `roar()`: every living thing alive in its box grown by 4 but a ravager (with
/// `mob_griefing` on, armour stands too) is hurt by its attack for 6 unless an illager,
/// and thrown back ([`strong_knockback`]) unless a player, each in turn; then its
/// `entity_action` and entity event 69, by which clients show the roar and push their
/// own player.
fn roar(tick: &mut MobTick<'_>) {
    let from = tick.living.body.position;
    let bounds = tick
        .living
        .body
        .bounds()
        .inflate([ROAR_REACH, ROAR_REACH, ROAR_REACH]);
    let mut writes = Vec::new();
    for seen in tick.world.entities_in(&bounds) {
        let looked = Looked::of_seen(&seen);
        let Some(kind) = looked.kind else {
            continue;
        };
        if !looked.mob
            || !looked.alive
            || looked.key == tick.key
            || i32::from(kind) == entity_types::RAVAGER
        {
            continue;
        }
        if !is_illager(kind) {
            writes.push(Local::Hurt {
                key: looked.key,
                kind: tick.kind,
                from,
                amount: ROAR_DAMAGE,
                sound: None,
                knockback: None,
                fling: None,
            });
        }
        writes.push(Local::Push {
            key: looked.key,
            by: strong_knockback(from, looked.position),
        });
    }
    tick.out.local.extend(writes);
    tick.out
        .events
        .push(Event::GameEvent(GameEvent::EntityAction));
    tick.out.events.push(Event::Entity(69));
}

/// `aiStep`'s break through leaves: with `mob_griefing` on (as everywhere) and its last
/// move stopped sideways, every leaves block in its box grown by 0.2 is broken with its
/// drops (`destroyBlock(pos, true, ravager)`), in `betweenClosed`'s order, x fastest,
/// then y, then z; where none was, it jumps if on the ground.
fn break_leaves(tick: &mut MobTick<'_>) {
    if !tick.living.body.horizontal_collision {
        return;
    }
    let bounds = tick.living.body.bounds().inflate([LEAVES_REACH; 3]);
    let floor = crate::physics::floor;
    let (min, max) = (bounds.min, bounds.max);
    let mut broke = false;
    for z in floor(min[2])..=floor(max[2]) {
        for y in floor(min[1])..=floor(max[1]) {
            for x in floor(min[0])..=floor(max[0]) {
                let pos = BlockPos::new(x, y, z);
                if is_class(tick.world.state(pos), BlockClass::LeavesBlock) {
                    tick.out.local.push(Local::BreakBlock { pos });
                    broke = true;
                }
            }
        }
    }
    if !broke && tick.living.body.on_ground {
        body::jump_from_ground(tick);
    }
}

impl MobType for Ravager {
    /// `Ravager.registerGoals`, after `Raider`'s, and its constructor's malus: leaves
    /// cost it nothing to path through.
    fn register_goals(&self, setup: &mut Setup) {
        illagers::register_raider_goals(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(4, RavagerAttackGoal);
        setup.add_goal(5, RandomStrollGoal::avoiding_water(0.4));
        setup.add_goal(6, LookAtPlayerGoal::new(Class::Player, 6.0));
        setup.add_goal(10, LookAtPlayerGoal::new(Class::Mob, 8.0));
        targets::register(setup, TARGETS);
        setup.set_malus(PathType::Leaves, 0.0);
    }

    /// `Raider.finalizeSpawn`: a ravager cannot lead (`canBeLeader`), but the leader's
    /// draw is made.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        illagers::finalize_spawn(entity_types::RAVAGER as u16, spawn, false);
    }

    /// `doHurtTarget`: its counter of 10, entity event 4, its attack sound, then
    /// `Mob`'s hit, of 12 and its attack knockback of 1.5.
    fn do_hurt_target(&self, ai: &mut Ai<'_, '_>, target: &Looked) {
        illagers::update_own(&mut ai.tick.mob, |own: &mut RavagerState| {
            own.attack_tick = ATTACK_DURATION;
        });
        ai.tick.out.events.push(Event::Entity(4));
        ai.tick.out.events.push(Event::Sound {
            sound: sounds::ENTITY_RAVAGER_ATTACK,
            volume: 1.0,
            pitch: 1.0,
        });
        crate::mobs::ai::goals::melee_hit(ai, target);
    }

    /// `isImmobile()`: dying, or any of its counters running.
    fn immobile(&self, tick: &MobTick<'_>) -> bool {
        if tick.mob.body.vitals.dying() {
            return true;
        }
        let own = state_of(&tick.mob);
        own.attack_tick > 0 || own.stunned_tick > 0 || own.roar_tick > 0
    }

    /// `Ravager.updateControlFlags`: a mob in its first seat that steers it is a raider
    /// (the only kind Clustine seats on it), whose steering leaves the ravager's goals
    /// their flags, where `Mob`'s would take its moves and looks; only a boat it rides
    /// takes its jumps. Undone here before its goals run, as the game never does it.
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        if !tick.controlled_by_mob {
            return;
        }
        let in_boat = tick.vehicle.is_some_and(crate::physics::entities::is_boat);
        let goals = &mut tick.mob.mind.goals.0;
        goals.disabled &= !(flags::MOVE | flags::LOOK);
        if !in_boat {
            goals.disabled &= !flags::JUMP;
        }
    }

    /// `Ravager.aiStep` after `Raider`'s: alive, its base speed nothing while immobile,
    /// else a tenth of the way to 0.35 with a target or 0.3 without; the leaves in its
    /// way; then its counters: the roar's (roaring at 10), the attack's, the stun's
    /// (its particle's draws each tick, and at its end the roar's sound and a roar's
    /// counter of 20).
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        if tick.mob.body.vitals.dying() {
            return;
        }
        let still = self.immobile(tick);
        let wanted = if tick.mob.body.combat.target.is_some() {
            ATTACK_SPEED
        } else {
            BASE_SPEED
        };
        if let Some(speed) = tick
            .mob
            .body
            .attributes
            .instance(tick.kind, attribute(ids::MOVEMENT_SPEED))
        {
            speed.base = if still {
                0.0
            } else {
                crate::physics::aabb::lerp(0.1, speed.base, wanted)
            };
        }
        break_leaves(tick);
        let mut own = state_of(&tick.mob);
        if own.roar_tick > 0 {
            own.roar_tick -= 1;
            if own.roar_tick == ROAR_AT {
                roar(tick);
            }
        }
        if own.attack_tick > 0 {
            own.attack_tick -= 1;
        }
        if own.stunned_tick > 0 {
            own.stunned_tick -= 1;
            stun_effect(tick);
            if own.stunned_tick == 0 {
                tick.out.events.push(Event::Sound {
                    sound: sounds::ENTITY_RAVAGER_ROAR,
                    volume: 1.0,
                    pitch: 1.0,
                });
                own.roar_tick = ROAR_DURATION;
            }
        }
        illagers::update_own(&mut tick.mob, |kept: &mut RavagerState| *kept = own);
    }

    /// `getMaxHeadYRot()`: 45.
    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_y_rot: 45,
            ..HeadLimits::default()
        }
    }

    fn remove_when_far_away(&self, _: u16, mob: &Mob, _: u32, distance: f64) -> bool {
        illagers::remove_when_far_away(mob, distance)
    }

    /// Of its `xpReward`, 20.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        illagers::experience(20, mob, draws)
    }

    /// `readAdditionalSaveData`: the raider's, and its three counters.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        illagers::from_template(mob, tag);
        illagers::update_own(mob, |own: &mut RavagerState| {
            own.attack_tick = super::template::int(tag, "AttackTick").unwrap_or(0);
            own.stunned_tick = super::template::int(tag, "StunTick").unwrap_or(0);
            own.roar_tick = super::template::int(tag, "RoarTick").unwrap_or(0);
        });
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_RAVAGER_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_RAVAGER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_RAVAGER_DEATH)
    }

    /// `playStepSound`: its own step, at 0.15 and 1.
    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_RAVAGER_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        illagers::shown(mob)
    }
}
