//! The shared behaviours (`ai.behavior`) the piglins' brains are made of that no brain
//! before them needed (family E5 of P12, `docs/groundwork/p-plan.md`, section 18), each
//! from the n7 dump of its class: `CopyMemoryWithExpiry`, `StartCelebratingIfTargetDead`,
//! `StopBeingAngryIfTargetDead`, `BackUpIfTooClose`,
//! `SetWalkTargetFromAttackTargetIfTargetOutOfReach` with a weapon's reach
//! (`BehaviorUtils.isWithinAttackRange`), `CrossbowAttack`, `GoToTargetLocation`,
//! `SetWalkTargetAwayFrom.pos`, `SetEntityLookTarget` of a predicate, `Mount` and
//! `DismountOrSkipMounting`.
//!
//! They are kept here, beside the piglins, so that the shared behaviours' files are not
//! touched by this family; they are written as any shared behaviour is, and a later brain
//! may take them from here.
//!
//! **Named**: `Mount` walks to the mob it would ride and stops within a block of it, as
//! the region has no mob's mount of another mob; and an entity the region's view does
//! not have is "unknown", of which nothing is judged (`docs/groundwork/n7j-plan.md`,
//! section 8 item 8).

use clustine_data::items;
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use crate::api::Hand;
use crate::items::crossbow;
use crate::mobs::Mob;
use crate::mobs::ai::goals::MeleeAttackGoal;
use crate::mobs::ai::ranged_weapons::{in_hand, is_holding, weapon_hand};
use crate::mobs::ai::sensing::Looked;
use crate::mobs::ai::{Ai, controls, random_pos};
use crate::mobs::body::using::{self, InUse};
use crate::mobs::brain::behaviours::{
    bottom_centre, entity_of, position_of, sample, tracking, walk, walk_target, walk_to_point,
};
use crate::mobs::brain::sensors::{find, find_closest_visible, visible_contains, visible_mobs};
use crate::mobs::brain::{
    Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Status, Timed, Tracker, Value,
};
use crate::physics::{self, Vec3};

/// `Entity.closerThan(other, distance)`: their feet nearer than `distance`.
pub(crate) fn closer_than(ai: &Ai<'_, '_>, target: &Looked, distance: f64) -> bool {
    target.distance_sqr(ai.position()) < distance * distance
}

/// `CrossbowItem.getDefaultProjectileRange()`.
pub(crate) const CROSSBOW_RANGE: i32 = 8;

/// `BehaviorUtils.isWithinAttackRange(mob, target, cooldown)` (n7 BehaviorUtils.txt):
/// with a crossbow in the main hand (the one projectile weapon a piglin may use,
/// `Piglin.canUseNonMeleeWeapon`), nearer than its range less `cooldown`; else within
/// the melee reach.
pub(crate) fn within_attack_range(ai: &Ai<'_, '_>, target: &Looked, cooldown: i32) -> bool {
    let crossbow =
        in_hand(&ai.tick.mob, Hand::Main).is_some_and(|stack| stack.item == items::CROSSBOW);
    if crossbow {
        return closer_than(ai, target, f64::from(CROSSBOW_RANGE - cooldown));
    }
    MeleeAttackGoal::within_range(ai, target)
}

/// `isHolding(CROSSBOW)`.
pub(crate) fn has_crossbow(mob: &Mob) -> bool {
    is_holding(mob, items::CROSSBOW)
}

// ------------------------------------------------------------------------------------
// `CopyMemoryWithExpiry`.

/// `CopyMemoryWithExpiry.create(predicate, from, to, duration)`: with `from` there and
/// `to` not, where the predicate holds, `to` takes `from`'s value for a time drawn of
/// the level's (`UniformInt.sample`).
pub(crate) struct CopyMemoryWithExpiry {
    pub(crate) predicate: fn(&mut Cx<'_, '_, '_>) -> bool,
    pub(crate) memories: [MemoryKind; 2],
    pub(crate) duration: (i32, i32),
}

/// [`CopyMemoryWithExpiry`], boxed.
pub(crate) fn copy_memory_with_expiry(
    predicate: fn(&mut Cx<'_, '_, '_>) -> bool,
    from: MemoryKind,
    to: MemoryKind,
    duration: (i32, i32),
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(CopyMemoryWithExpiry {
        predicate,
        memories: [from, to],
        duration,
    }))
}

impl Shot for CopyMemoryWithExpiry {
    fn required(&self) -> &[MemoryKind] {
        &self.memories
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let [from, to] = self.memories;
        if !cx.memories.check(from, Status::Present) || !cx.memories.check(to, Status::Absent) {
            return false;
        }
        if !(self.predicate)(cx) {
            return false;
        }
        let Some(value) = cx.memories.get(from) else {
            return false;
        };
        let ttl = sample(cx.ai.tick.draws, self.duration);
        cx.memories.set_with_expiry(to, value, i64::from(ttl));
        true
    }
}

// ------------------------------------------------------------------------------------
// Celebrating, and anger at the dead.

/// `StartCelebratingIfTargetDead.create(duration, dancePredicate)` (n7): an attack
/// target dead or dying is celebrated for `duration` ticks at its block, danced to where
/// the predicate holds (with the mob and the target), and forgotten with the anger at it
/// (`forgive_dead_players` holds).
pub(crate) struct StartCelebratingIfTargetDead {
    pub(crate) duration: i64,
    pub(crate) dances: fn(&mut Cx<'_, '_, '_>, &Looked) -> bool,
}

impl Shot for StartCelebratingIfTargetDead {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::AttackTarget,
            MemoryKind::AngryAt,
            MemoryKind::CelebrateLocation,
            MemoryKind::Dancing,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::AttackTarget, Status::Present)
            || !cx.memories.check(MemoryKind::AngryAt, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::CelebrateLocation, Status::Absent)
            || !cx.memories.check(MemoryKind::Dancing, Status::Registered)
        {
            return false;
        }
        let Some(target) = entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
        else {
            return false;
        };
        if target.alive {
            return false;
        }
        if (self.dances)(cx, &target) {
            cx.memories
                .set_with_expiry(MemoryKind::Dancing, Value::Bool(true), self.duration);
        }
        cx.memories.set_with_expiry(
            MemoryKind::CelebrateLocation,
            Value::Block(target.block_position()),
            self.duration,
        );
        // Not a player, or a player whom `forgive_dead_players` (on) forgives.
        cx.memories.erase(MemoryKind::AttackTarget);
        cx.memories.erase(MemoryKind::AngryAt);
        true
    }
}

/// The living thing of the UUID `uuid` as the mob's view has it: a player of the
/// region's or a face it keeps, else a mob among those it lives near (`mobs`), which is
/// where any it could still be angry at stands (its follow range); none where the view
/// has it not ("unknown"). `BehaviorUtils.getLivingEntityFromUUIDMemory`'s lookup.
pub(crate) fn living_of_uuid(cx: &Cx<'_, '_, '_>, uuid: u128) -> Option<Looked> {
    let world = cx.ai.tick.world;
    if let Some(player) = world
        .players()
        .into_iter()
        .find(|player| player.uuid == uuid)
    {
        return find(cx.ai, player.key);
    }
    let Some(Value::Entities(near)) = cx.memories.get(MemoryKind::Mobs) else {
        return None;
    };
    near.into_iter()
        .find(|key| {
            world
                .entity(*key)
                .is_some_and(|seen| seen.entity.uuid.as_u128() == uuid)
        })
        .and_then(|key| find(cx.ai, key))
}

/// The UUID of the thing `key` names: a player's, or an entity's of the view.
pub(crate) fn uuid_of(cx: &Cx<'_, '_, '_>, key: EntityId) -> Option<u128> {
    let world = cx.ai.tick.world;
    if let Some(player) = world.players().into_iter().find(|player| player.key == key) {
        return Some(player.uuid);
    }
    world.entity(key).map(|seen| seen.entity.uuid.as_u128())
}

/// `StopBeingAngryIfTargetDead.create()` (n7): the anger at a living thing that is dead
/// or dying forgotten (a player too, `forgive_dead_players` being on).
pub(crate) struct StopBeingAngryIfTargetDead;

impl Shot for StopBeingAngryIfTargetDead {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::AngryAt]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(Value::Uuid(uuid)) = cx.memories.get(MemoryKind::AngryAt) else {
            return false;
        };
        if living_of_uuid(cx, uuid).is_some_and(|target| !target.alive) {
            cx.memories.erase(MemoryKind::AngryAt);
        }
        true
    }
}

// ------------------------------------------------------------------------------------
// Fighting.

/// `BackUpIfTooClose.create(tooCloseDistance, strafeSpeed)` (n7): with no walk target,
/// an attack target seen nearer than the distance is looked at and backed away from
/// (`strafe(-speed, 0)`), the body turned to the head (`rotateIfNecessary(yRot,
/// yHeadRot, 0)`).
pub(crate) struct BackUpIfTooClose {
    pub(crate) distance: i32,
    pub(crate) speed: f32,
}

impl Shot for BackUpIfTooClose {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::WalkTarget,
            MemoryKind::LookTarget,
            MemoryKind::AttackTarget,
            MemoryKind::VisibleMobs,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
            || !cx.memories.check(MemoryKind::AttackTarget, Status::Present)
            || !cx.memories.check(MemoryKind::VisibleMobs, Status::Present)
        {
            return false;
        }
        let Some(target) = entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
        else {
            return false;
        };
        if !closer_than(cx.ai, &target, f64::from(self.distance))
            || !visible_contains(cx, target.key)
        {
            return false;
        }
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(target.key, true)),
        );
        controls::strafe(cx.ai, -self.speed, 0.0);
        cx.ai.tick.yaw = cx.ai.tick.mob.body.head.head_yaw;
        true
    }
}

/// `SetWalkTargetFromAttackTargetIfTargetOutOfReach.create(speed)` (n7): within the
/// weapon's reach of a seen attack target (`isWithinAttackRange(mob, target, 1)`), no
/// walk; else a look at it and a walk to it.
pub(crate) struct WalkToAttackTarget {
    pub(crate) speed: f32,
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
            && within_attack_range(cx.ai, &target, 1);
        if near {
            cx.memories.erase(MemoryKind::WalkTarget);
        } else {
            cx.memories.set(
                MemoryKind::LookTarget,
                Value::Tracker(tracking(target.key, true)),
            );
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk(tracking(target.key, false), self.speed, 0),
            );
        }
        true
    }
}

/// `CrossbowAttack$CrossbowState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CrossbowState {
    #[default]
    Uncharged,
    Charging,
    Charged,
    ReadyToAttack,
}

/// What `CrossbowAttack` keeps: its `attackDelay` and its state, kept between its runs
/// as the game's fields are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Crossbowing {
    pub(crate) delay: i32,
    pub(crate) state: CrossbowState,
}

/// `CrossbowAttack()` (n7 CrossbowAttack.txt): for 1200 ticks at most, while it holds a
/// crossbow and sees its attack target within the crossbow's range, it looks at the
/// target and charges its crossbow by using it (`setChargingCrossbow`), waits 20 to 39
/// ticks once charged (`nextInt(20)` of its own random), and shoots
/// (`performRangedAttack(target, 1)`, the type's). Its stop ends a use under way and
/// the charging flag.
pub(crate) struct CrossbowAttack {
    /// `RangedAttackMob.performRangedAttack(target, power)` of the type.
    pub(crate) perform: fn(&mut Ai<'_, '_>, &Looked),
    /// `CrossbowAttackMob.setChargingCrossbow(charging)` of the type.
    pub(crate) set_charging: fn(&mut Mob, bool),
}

/// [`CrossbowAttack`], boxed.
pub(crate) fn crossbow_attack(
    perform: fn(&mut Ai<'_, '_>, &Looked),
    set_charging: fn(&mut Mob, bool),
) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(CrossbowAttack {
        perform,
        set_charging,
    }))
}

impl CrossbowAttack {
    /// `checkExtraStartConditions`: a crossbow, the target seen and within its range.
    fn may(cx: &mut Cx<'_, '_, '_>) -> Option<Looked> {
        let target = entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))?;
        (has_crossbow(&cx.ai.tick.mob)
            && visible_contains(cx, target.key)
            && within_attack_range(cx.ai, &target, 0))
        .then_some(target)
    }
}

impl Timed for CrossbowAttack {
    type State = Crossbowing;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::AttackTarget, Status::Present),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (1200, 1200)
    }

    fn extra_start(&self, _: &mut Crossbowing, cx: &mut Cx<'_, '_, '_>) -> bool {
        Self::may(cx).is_some()
    }

    fn can_still_use(&self, _: &mut Crossbowing, cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories.check(MemoryKind::AttackTarget, Status::Present) && Self::may(cx).is_some()
    }

    fn tick(&self, state: &mut Crossbowing, cx: &mut Cx<'_, '_, '_>) {
        let Some(target) = entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
        else {
            return;
        };
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(target.key, true)),
        );
        let ai = &mut *cx.ai;
        match state.state {
            CrossbowState::Uncharged => {
                let hand = weapon_hand(&ai.tick.mob, items::CROSSBOW);
                using::start_using(ai.tick, hand);
                state.state = CrossbowState::Charging;
                (self.set_charging)(&mut ai.tick.mob, true);
            }
            CrossbowState::Charging => {
                if !using::is_using(&ai.tick.mob) {
                    state.state = CrossbowState::Uncharged;
                }
                let ticks = using::ticks_using(&ai.tick.mob);
                // `getUseItem()`: the stack in use, the empty one where none is.
                let use_item = ai
                    .tick
                    .mob
                    .body
                    .using
                    .using
                    .and_then(|InUse { hand, .. }| in_hand(&ai.tick.mob, hand).cloned())
                    .unwrap_or(clustine_world::ItemStack::new(items::AIR, 0));
                if ticks >= crossbow::charge_duration(&use_item) {
                    using::release_using(ai.tick);
                    state.state = CrossbowState::Charged;
                    state.delay = 20 + ai.tick.draws.next_int(20) as i32;
                    (self.set_charging)(&mut ai.tick.mob, false);
                }
            }
            CrossbowState::Charged => {
                state.delay -= 1;
                if state.delay == 0 {
                    state.state = CrossbowState::ReadyToAttack;
                }
            }
            CrossbowState::ReadyToAttack => {
                (self.perform)(ai, &target);
                state.state = CrossbowState::Uncharged;
            }
        }
    }

    fn stop(&self, _: &mut Crossbowing, cx: &mut Cx<'_, '_, '_>) {
        if using::is_using(&cx.ai.tick.mob) {
            using::stop_using(cx.ai.tick);
        }
        if has_crossbow(&cx.ai.tick.mob) {
            (self.set_charging)(&mut cx.ai.tick.mob, false);
            // `getUseItem().set(CHARGED_PROJECTILES, EMPTY)`: the use has stopped, so the
            // use item is the empty one, which this changes nothing of.
        }
    }
}

// ------------------------------------------------------------------------------------
// Walking.

/// `GoToTargetLocation.create(memory, closeEnough, speed)` (n7): with no attack target
/// and no walk target, a walk and a look to within a block of the place `memory` holds
/// (`getNearbyPos`, an offset of `nextInt(3) − 1` each way of the level's), where the
/// mob is not within `close` of it already.
pub(crate) struct GoToTargetLocation {
    pub(crate) memories: [MemoryKind; 4],
    pub(crate) close: i32,
    pub(crate) speed: f32,
}

/// [`GoToTargetLocation`] of `memory`, boxed.
pub(crate) fn go_to_target_location(
    memory: MemoryKind,
    close: i32,
    speed: f32,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(GoToTargetLocation {
        memories: [
            memory,
            MemoryKind::AttackTarget,
            MemoryKind::WalkTarget,
            MemoryKind::LookTarget,
        ],
        close,
        speed,
    }))
}

impl Shot for GoToTargetLocation {
    fn required(&self) -> &[MemoryKind] {
        &self.memories
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let memory = self.memories[0];
        if !cx.memories.check(memory, Status::Present)
            || !cx.memories.check(MemoryKind::AttackTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::WalkTarget, Status::Absent)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
        {
            return false;
        }
        let Some(Value::Block(pos)) = cx.memories.get(memory) else {
            return false;
        };
        let at = cx.ai.block_position();
        let close = f64::from(self.close);
        if crate::mobs::brain::sensors::block_distance_sqr(pos, at) < close * close {
            return true;
        }
        let draws = &mut *cx.ai.tick.draws;
        let dx = draws.next_int(3) as i32 - 1;
        let dz = draws.next_int(3) as i32 - 1;
        let near = BlockPos::new(pos.x + dx, pos.y, pos.z + dz);
        let tracker = Tracker::Block(near);
        cx.memories
            .set(MemoryKind::LookTarget, Value::Tracker(tracker));
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(tracker, self.speed, self.close),
        );
        true
    }
}

/// `SetWalkTargetAwayFrom.pos(memory, speed, desiredDistance, overrideWalkTarget)` (n7):
/// as the entity's form (`brain/behaviours/fleeing.rs`), from the bottom middle of the
/// block `memory` holds.
pub(crate) struct WalkAwayFromPos {
    pub(crate) memories: [MemoryKind; 2],
    pub(crate) speed: f32,
    pub(crate) distance: i32,
    pub(crate) override_walk: bool,
}

/// [`WalkAwayFromPos`] of `memory`, boxed.
pub(crate) fn away_from_pos(
    memory: MemoryKind,
    speed: f32,
    distance: i32,
    override_walk: bool,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(WalkAwayFromPos {
        memories: [MemoryKind::WalkTarget, memory],
        speed,
        distance,
        override_walk,
    }))
}

impl Shot for WalkAwayFromPos {
    fn required(&self) -> &[MemoryKind] {
        &self.memories
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let memory = self.memories[1];
        if !cx
            .memories
            .check(MemoryKind::WalkTarget, Status::Registered)
            || !cx.memories.check(memory, Status::Present)
        {
            return false;
        }
        let existing = walk_target(cx);
        if existing.is_some() && !self.override_walk {
            return false;
        }
        let Some(Value::Block(pos)) = cx.memories.get(memory) else {
            return false;
        };
        let target: Vec3 = bottom_centre(pos);
        let position = cx.ai.position();
        let reach = f64::from(self.distance);
        if physics::distance_sqr(position, target) >= reach * reach {
            return false;
        }
        if let Some(walk) = existing
            && walk.speed.to_bits() == self.speed.to_bits()
            && let Some(to) = position_of(cx, &walk.to)
        {
            let along = physics::sub(to, position);
            let towards = physics::sub(target, position);
            let dot = along[0] * towards[0] + along[1] * towards[1] + along[2] * towards[2];
            if dot < 0.0 {
                return false;
            }
        }
        for _ in 0..10 {
            if let Some(at) = random_pos::land_pos_away(cx.ai, 16, 7, target) {
                cx.memories
                    .set(MemoryKind::WalkTarget, walk_to_point(at, self.speed, 0));
                break;
            }
        }
        true
    }
}

// ------------------------------------------------------------------------------------
// Looking.

/// `SetEntityLookTarget.create(predicate, maxDist)` (n7): with no look target, the
/// closest seen thing within `max_distance` that the predicate takes and that does not
/// ride the mob, looked at. No draws.
pub(crate) struct LookAtBy {
    pub(crate) predicate: fn(&Looked) -> bool,
    pub(crate) max_distance: f32,
}

/// [`LookAtBy`], boxed.
pub(crate) fn look_at_by(predicate: fn(&Looked) -> bool, max_distance: f32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(LookAtBy {
        predicate,
        max_distance,
    }))
}

impl Shot for LookAtBy {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::LookTarget, MemoryKind::VisibleMobs]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::LookTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::VisibleMobs, Status::Present)
        {
            return false;
        }
        // `maxDist * maxDist` in float, widened at the comparison.
        let most = f64::from(self.max_distance * self.max_distance);
        let own = cx.ai.tick.key;
        let from = cx.ai.position();
        let predicate = self.predicate;
        let Some(target) = find_closest_visible(cx, &mut |_, looked| {
            predicate(looked) && looked.distance_sqr(from) <= most && looked.vehicle != Some(own)
        }) else {
            return false;
        };
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(target.key, true)),
        );
        true
    }
}

/// `BehaviorBuilder.triggerIf(e -> true)`: a one-shot that always fires.
pub(crate) struct Always;

impl Shot for Always {
    fn required(&self) -> &[MemoryKind] {
        &[]
    }

    fn trigger(&self, _: &mut Cx<'_, '_, '_>) -> bool {
        true
    }
}

// ------------------------------------------------------------------------------------
// Riding.

/// `Mount.create(speed)` (n7): not riding, a ride target walked to and looked at, to
/// within a block; there it would ride it (see the module's head).
pub(crate) struct Mount {
    pub(crate) speed: f32,
}

impl Shot for Mount {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::LookTarget,
            MemoryKind::WalkTarget,
            MemoryKind::RideTarget,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::LookTarget, Status::Registered)
            || !cx.memories.check(MemoryKind::WalkTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::RideTarget, Status::Present)
        {
            return false;
        }
        if cx.ai.tick.vehicle.is_some() {
            return false;
        }
        let Some(target) = entity_of(cx, MemoryKind::RideTarget).and_then(|key| find(cx.ai, key))
        else {
            return false;
        };
        if !closer_than(cx.ai, &target, 1.0) {
            cx.memories.set(
                MemoryKind::LookTarget,
                Value::Tracker(tracking(target.key, true)),
            );
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk(tracking(target.key, false), self.speed, 1),
            );
        }
        true
    }
}

/// `DismountOrSkipMounting.create(maxDistance, wantsToDismount)` (n7): of what it rides,
/// else its ride target, one dead, out of reach or that it no longer wants
/// (`wants_off`, with the mob and that entity) is left, and the ride target forgotten.
pub(crate) struct DismountOrSkipMounting {
    pub(crate) distance: i32,
    pub(crate) wants_off: fn(&mut Cx<'_, '_, '_>, &Looked) -> bool,
}

impl Shot for DismountOrSkipMounting {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::RideTarget]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::RideTarget, Status::Registered)
        {
            return false;
        }
        // The mob rides nothing in Clustine's view of a brain (see the module's head).
        let Some(key) = entity_of(cx, MemoryKind::RideTarget) else {
            return false;
        };
        let Some(other) = find(cx.ai, key) else {
            return false;
        };
        let valid = other.alive && closer_than(cx.ai, &other, f64::from(self.distance));
        if !valid || (self.wants_off)(cx, &other) {
            cx.memories.erase(MemoryKind::RideTarget);
            return true;
        }
        false
    }
}
