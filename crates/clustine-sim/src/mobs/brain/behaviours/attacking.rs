//! Attacking (the brief's 7.3.20): `StartAttacking`, `StopAttackingIfTargetInvalid`,
//! `MeleeAttack` and `SetWalkTargetFromAttackTargetIfTargetOutOfReach`.
//!
//! **Named**: no mob of these holds a weapon that shoots, so the projectile weapon's
//! range and `canUseNonMeleeWeapon` are never asked; the arm's swing
//! (`swingForAttack`) is not shown, as `MeleeAttackGoal`'s is not.

use clustine_world::EntityId;

use super::super::sensors::{find, visible_contains, visible_mobs};
use super::super::{Behaviour, Cx, HeldShot, MemoryKind, Shot, Status, Value};
use super::{Speed, entity_of, tracking, walk};
use crate::mobs::ai::goals::{MeleeAttackGoal, melee_hit};
use crate::mobs::ai::sensing::{Looked, can_attack};

fn always(_: &mut Cx<'_, '_, '_>) -> bool {
    true
}

/// `StartAttacking.create([condition,] finder)`: with no attack target, the one the
/// finder gives, where the mob may attack it; `cant_reach_walk_target_since` forgotten.
pub(crate) struct StartAttacking {
    pub(crate) condition: fn(&mut Cx<'_, '_, '_>) -> bool,
    pub(crate) finder: fn(&mut Cx<'_, '_, '_>) -> Option<EntityId>,
}

/// `StartAttacking.create(finder)`.
pub(crate) fn start_attacking(
    finder: fn(&mut Cx<'_, '_, '_>) -> Option<EntityId>,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(StartAttacking {
        condition: always,
        finder,
    }))
}

impl Shot for StartAttacking {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::AttackTarget,
            MemoryKind::CantReachWalkTargetSince,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::AttackTarget, Status::Absent)
            || !cx
                .memories
                .check(MemoryKind::CantReachWalkTargetSince, Status::Registered)
        {
            return false;
        }
        if !(self.condition)(cx) {
            return false;
        }
        let Some(target) = (self.finder)(cx).and_then(|key| find(cx.ai, key)) else {
            return false;
        };
        if !can_attack(&target) {
            return false;
        }
        cx.memories
            .set(MemoryKind::AttackTarget, Value::Entity(target.key));
        cx.memories.erase(MemoryKind::CantReachWalkTargetSince);
        true
    }
}

fn never(_: &mut Cx<'_, '_, '_>, _: &Looked) -> bool {
    false
}

fn no_callback(_: &mut Cx<'_, '_, '_>, _: EntityId) {}

/// `StopAttackingIfTargetInvalid.create([condition,] [callback,] [tired])`: the attack
/// target forgotten where the mob may not attack it, it has been out of reach for more
/// than 200 ticks (`tired`), it is dead, or the condition holds.
///
/// A target the view has not ("unknown") is kept and nothing is judged this tick: it is
/// not dead nor in another level (the plan, section 8 item 8).
pub(crate) struct StopAttackingIfTargetInvalid {
    pub(crate) condition: fn(&mut Cx<'_, '_, '_>, &Looked) -> bool,
    pub(crate) callback: fn(&mut Cx<'_, '_, '_>, EntityId),
    pub(crate) tired: bool,
}

/// `StopAttackingIfTargetInvalid.create()`.
pub(crate) fn stop_attacking() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(StopAttackingIfTargetInvalid {
        condition: never,
        callback: no_callback,
        tired: true,
    }))
}

impl Shot for StopAttackingIfTargetInvalid {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::AttackTarget,
            MemoryKind::CantReachWalkTargetSince,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::AttackTarget, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::CantReachWalkTargetSince, Status::Registered)
        {
            return false;
        }
        let Some(key) = entity_of(cx, MemoryKind::AttackTarget) else {
            return false;
        };
        let Some(target) = find(cx.ai, key) else {
            return true;
        };
        let tired = self.tired
            && matches!(
                cx.memories.get(MemoryKind::CantReachWalkTargetSince),
                Some(Value::Long(since)) if cx.time - since > 200
            );
        if !can_attack(&target) || tired || !target.alive || (self.condition)(cx, &target) {
            (self.callback)(cx, key);
            cx.memories.erase(MemoryKind::AttackTarget);
        }
        true
    }
}

fn hit_as_mob(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    melee_hit(cx.ai, target);
}

fn any(_: &Cx<'_, '_, '_>) -> bool {
    true
}

/// `MeleeAttack.create([predicate,] cooldown)`: a seen attack target within reach is
/// looked at and hit (`doHurtTarget`, a type's own where it has one), and the attack
/// cools down for `cooldown` ticks.
pub(crate) struct MeleeAttack {
    pub(crate) predicate: fn(&Cx<'_, '_, '_>) -> bool,
    pub(crate) cooldown: i64,
    /// `doHurtTarget(level, target)`.
    pub(crate) hit: fn(&mut Cx<'_, '_, '_>, &Looked),
}

/// `MeleeAttack.create(cooldown)`.
pub(crate) fn melee(cooldown: i64) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(MeleeAttack {
        predicate: any,
        cooldown,
        hit: hit_as_mob,
    }))
}

impl Shot for MeleeAttack {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::LookTarget,
            MemoryKind::AttackTarget,
            MemoryKind::AttackCoolingDown,
            MemoryKind::VisibleMobs,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::LookTarget, Status::Registered)
            || !cx.memories.check(MemoryKind::AttackTarget, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::AttackCoolingDown, Status::Absent)
            || !cx.memories.check(MemoryKind::VisibleMobs, Status::Present)
        {
            return false;
        }
        let Some(target) = entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
        else {
            return false;
        };
        if !(self.predicate)(cx)
            || !MeleeAttackGoal::within_range(cx.ai, &target)
            || !visible_contains(cx, target.key)
        {
            return false;
        }
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(target.key, true)),
        );
        // `swing(MAIN_HAND)`, then `doHurtTarget`.
        cx.ai.tick.out.local.push(crate::mobs::tick::Local::Swing);
        (self.hit)(cx, &target);
        cx.memories.set_with_expiry(
            MemoryKind::AttackCoolingDown,
            Value::Bool(true),
            self.cooldown,
        );
        true
    }
}

/// `SetWalkTargetFromAttackTargetIfTargetOutOfReach.create(speed)`: within reach of a
/// seen attack target, no walk; else a look at it and a walk to it. A target the view has
/// not is not walked to this tick.
pub(crate) struct WalkToAttackTarget {
    pub(crate) speed: Speed,
}

/// [`WalkToAttackTarget`], boxed.
pub(crate) fn walk_to_attack_target(speed: Speed) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(WalkToAttackTarget { speed }))
}

impl Shot for WalkToAttackTarget {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::WalkTarget,
            MemoryKind::LookTarget,
            MemoryKind::AttackTarget,
            MemoryKind::VisibleMobs,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::WalkTarget, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
            || !cx.memories.check(MemoryKind::AttackTarget, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::VisibleMobs, Status::Registered)
        {
            return false;
        }
        let Some(target) = entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
        else {
            return false;
        };
        let near = visible_mobs(cx.memories).is_some()
            && visible_contains(cx, target.key)
            && MeleeAttackGoal::within_range(cx.ai, &target);
        if near {
            cx.memories.erase(MemoryKind::WalkTarget);
        } else {
            cx.memories.set(
                MemoryKind::LookTarget,
                Value::Tracker(tracking(target.key, true)),
            );
            let speed = self.speed.of(cx);
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk(tracking(target.key, false), speed, 0),
            );
        }
        true
    }
}
