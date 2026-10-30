//! The piglin's brain (`PiglinAi`, `Piglin.BRAIN_PROVIDER`; PiglinAi.txt of the d dump,
//! with `PiglinSpecificSensor`, `StartAdmiringItemIfSeen`, `StopAdmiringIfItemTooFarAway`,
//! `StopAdmiringIfTiredOfTryingToReachItem`, `StopHoldingItemIfNoLongerAdmiring`,
//! `StartHuntingHoglin` and `RememberIfHoglinWasKilled`): its provider, its seven
//! activities (core, idle, admiring an item, fighting, celebrating, retreating and riding),
//! its sensors, and what `PiglinAi` does outside the brain's tick: a player's gift, the
//! pick-up, the hurt's `wasHurtBy` and the anger it spreads (`broadcastAngerTarget`,
//! `broadcastRetreat`), and `angerNearbyPiglins`' anger (`region/telling.rs`).
//!
//! **What comes between ticks.** The game runs `wasHurtBy` as the hurt lands and
//! `angerNearbyPiglins` and `broadcastAngerTarget` as they are called, each reading the
//! piglin's sensed memories and the world. Clustine's hurt and telling see the piglin
//! alone, so each is kept ([`super::piglin::Pending`]) and taken at the start of the
//! piglin's next AI step, before its brain's tick, as of its last tick (the moment a
//! write between ticks has): what its brain then reads is what the game's would read.
//! Its sight of a player who opened a container is judged where it is told
//! (`Region::tell_mob`), and whether it is idle too. **Named**: what was told is judged
//! against the world as the piglin's tick begins rather than as it was told (a tick
//! apart at most); a piglin in another region than its teller is told through the
//! region that holds it (`EntityUse::Told`), the retreat's writes into other piglins
//! reach only the region's own (`Local::SetMemories`).
//!
//! Every player is a creative one (`mobs/ai/sensing.rs`), whom no piglin may attack:
//! anger at a player (`setAngerTarget`) is refused by `isEntityAttackableIgnoringLineOf
//! Sight` as the game's is, so that it shows only for a player of another game mode.
//!
//! Named differences besides: the spear's three behaviours of the fight
//! (`SpearApproach`, `SpearAttack`, `SpearRetreat`) hold their places and never start;
//! a player's armour is not read (`SeenPlayer` carries none), which no creative player
//! needs (`canAttack` refuses them before); the hoglins' records are read for their age
//! and whether they may be hunted (`MobType::cannot_be_hunted`), and a hoglin's own hurt
//! is not read (its `HURT_BY` is sensed, which the record lacks).

use clustine_blocks::data::{flag, in_tag};
use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, Double, EntityData, EntityId, ItemStack, Told, ToldWhat};

use super::piglin::{
    self, Other, PIGLIN, PIGLIN_BRUTE, Pending, hold_in_main_hand, hold_in_off_hand,
    is_barter_currency, is_food, is_loved, not_holding_loved_in_off_hand, off_hand, read, update,
};
use super::piglin_behaviours::{
    Always, BackUpIfTooClose, CopyMemoryWithExpiry, DismountOrSkipMounting, Mount,
    StartCelebratingIfTargetDead, StopBeingAngryIfTargetDead, WalkToAttackTarget, away_from_pos,
    closer_than, copy_memory_with_expiry, crossbow_attack, go_to_target_location, has_crossbow,
    living_of_uuid, look_at_by, uuid_of,
};
use super::{Did, Interaction, Used};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::melee_hit;
use crate::mobs::ai::ranged_weapons;
use crate::mobs::ai::sensing::{self, Looked};
use crate::mobs::ai::{Ai, Setup, random_pos};
use crate::mobs::body;
use crate::mobs::brain::behaviours::attacking::{
    MeleeAttack, StartAttacking, StopAttackingIfTargetInvalid,
};
use crate::mobs::brain::behaviours::cooldowns::erase_if;
use crate::mobs::brain::behaviours::fleeing::away_from;
use crate::mobs::brain::behaviours::items::{GoToWantedItem, go_to_wanted_item};
use crate::mobs::brain::behaviours::looking::{LookAt, Ticker, look_at};
use crate::mobs::brain::behaviours::social_villagers::{interact_with, set_look_and_interact};
use crate::mobs::brain::behaviours::walking::{
    StrollTo, look_sink, sink, stroll, stroll_to, walk_to_look_target,
};
use crate::mobs::brain::behaviours::{
    Counter, HeldKept, KeptShot, doors, entity_of, sample, trigger_if,
};
use crate::mobs::brain::sensors::{
    self, attackable, attackable_ignoring_line_of_sight, find, is_baby, targetable, visible_mobs,
};
use crate::mobs::brain::{
    self, Activity, ActivityData, Behaviour, Brain, BrainState, Cx, DoNothing, Gate, HeldShot,
    HeldTimed, Memories, MemoryKind, MemoryWrite, Provider, Sensor, Shot, Status, Value,
};
use crate::mobs::part;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::Vec3;

/// `TIME_BETWEEN_HUNTS`: 30 to 120 seconds.
pub(crate) const HUNTS: (i32, i32) = (600, 2400);
/// `RIDE_START_INTERVAL`, `RIDE_DURATION`, `RETREAT_DURATION`, `AVOID_ZOMBIFIED_DURATION`
/// and `BABY_AVOID_NEMESIS_DURATION`, in ticks.
const RIDE_START_INTERVAL: (i32, i32) = (200, 800);
const RIDE_DURATION: (i32, i32) = (200, 600);
const RETREAT_DURATION: (i32, i32) = (100, 400);
const AVOID_ZOMBIFIED_DURATION: (i32, i32) = (100, 140);
const BABY_AVOID_NEMESIS_DURATION: (i32, i32) = (100, 140);
/// `ANGER_DURATION`, `ADMIRE_DURATION`, `CELEBRATION_TIME`, `EAT_COOLDOWN`,
/// `HIT_BY_PLAYER_MEMORY_TIMEOUT`, `BABY_FLEE_DURATION_AFTER_GETTING_HIT`.
pub(crate) const ANGER_DURATION: i64 = 600;
pub(crate) const ADMIRE_DURATION: i64 = 119;
const CELEBRATION_TIME: i64 = 300;
const EAT_COOLDOWN: i64 = 200;
const HIT_BY_PLAYER_MEMORY_TIMEOUT: i64 = 400;
const BABY_FLEE_DURATION: i64 = 100;
/// `MAX_DISTANCE_TO_WALK_TO_ITEM`, `MAX_TIME_TO_WALK_TO_ITEM`,
/// `HOW_LONG_TIME_TO_DISABLE_ADMIRE_WALKING_IF_CANT_REACH_ITEM`.
const ITEM_REACH: i32 = 9;
const ITEM_WALK_TIME: i32 = 200;
const ITEM_WALK_DISABLED: i64 = 200;
/// `DESIRED_DISTANCE_FROM_ZOMBIFIED`, `DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING`.
const ZOMBIFIED_DISTANCE: f64 = 6.0;
const AVOIDING_DISTANCE: f64 = 12.0;
/// `REPELLENT_DETECTION_RANGE_HORIZONTAL` and `_VERTICAL`.
const REPELLENT_REACH: (i32, i32) = (8, 4);

const HOGLIN: u16 = entity_types::HOGLIN as u16;
const ZOMBIFIED_PIGLIN: u16 = entity_types::ZOMBIFIED_PIGLIN as u16;
const ZOGLIN: u16 = entity_types::ZOGLIN as u16;
const WITHER_SKELETON: u16 = entity_types::WITHER_SKELETON as u16;
const WITHER: u16 = entity_types::WITHER as u16;
const PLAYER: u16 = entity_types::PLAYER as u16;

// ------------------------------------------------------------------------------------
// What a piglin reads of itself and of others.

/// `isBaby()` of the mob of the step.
fn baby(cx: &Cx<'_, '_, '_>) -> bool {
    piglin::is_baby(&cx.ai.tick.mob)
}

fn adult(cx: &mut Cx<'_, '_, '_>) -> bool {
    !baby(cx)
}

fn baby_cx(cx: &mut Cx<'_, '_, '_>) -> bool {
    baby(cx)
}

/// `canHunt()`: the piglin's `!cannotHunt`; never a brute.
fn can_hunt(cx: &Cx<'_, '_, '_>) -> bool {
    cx.ai.tick.kind == PIGLIN && !read(&cx.ai.tick.mob).cannot_hunt
}

fn can_hunt_cx(cx: &mut Cx<'_, '_, '_>) -> bool {
    can_hunt(cx)
}

fn holds_crossbow(cx: &mut Cx<'_, '_, '_>) -> bool {
    has_crossbow(&cx.ai.tick.mob)
}

fn dancing(cx: &mut Cx<'_, '_, '_>) -> bool {
    read(&cx.ai.tick.mob).dancing
}

fn not_dancing(cx: &mut Cx<'_, '_, '_>) -> bool {
    !dancing(cx)
}

fn is_passenger(cx: &mut Cx<'_, '_, '_>) -> bool {
    cx.ai.tick.vehicle.is_some()
}

/// `isZombified(entity)`: a zombified piglin or a zoglin.
fn is_zombified(looked: &Looked) -> bool {
    matches!(looked.kind, Some(ZOMBIFIED_PIGLIN | ZOGLIN))
}

/// `isPlayerHoldingLovedItem(entity)`: a player with a loved item in either hand.
pub(crate) fn player_holding_loved(looked: &Looked) -> bool {
    looked.is_player()
        && [&looked.main_hand, &looked.off_hand]
            .into_iter()
            .flatten()
            .any(|stack| stack.count > 0 && is_loved(stack))
}

/// `seesPlayerHoldingLovedItem(piglin)`.
fn sees_player_holding_loved(memories: &Memories) -> bool {
    memories.check(MemoryKind::NearestPlayerHoldingWantedItem, Status::Present)
}

fn sees_no_player_holding_loved(cx: &mut Cx<'_, '_, '_>) -> bool {
    !sees_player_holding_loved(cx.memories)
}

/// `isNearZombified(piglin)`: the nearest zombified thing it sees within 6. One the view
/// has not is taken as not near.
fn is_near_zombified(cx: &mut Cx<'_, '_, '_>) -> bool {
    entity_of(cx, MemoryKind::NearestVisibleZombified)
        .and_then(|key| find(cx.ai, key))
        .is_some_and(|zombified| closer_than(cx.ai, &zombified, ZOMBIFIED_DISTANCE))
}

/// `isAdmiringDisabled(piglin)`.
fn admiring_disabled(memories: &Memories) -> bool {
    memories.check(MemoryKind::AdmiringDisabled, Status::Present)
}

/// `isAdmiringItem(piglin)`.
fn admiring(memories: &Memories) -> bool {
    memories.check(MemoryKind::AdmiringItem, Status::Present)
}

/// `hasEatenRecently(piglin)`.
fn ate_recently(memories: &Memories) -> bool {
    memories.check(MemoryKind::AteRecently, Status::Present)
}

/// The active activity beside the core one as the record has it: the default, idle,
/// before its first tick.
pub(crate) fn recorded_activity(mob: &Mob) -> Activity {
    part::decode::<BrainState>(&mob.mind.brain)
        .ok()
        .and_then(|state| state.active)
        .unwrap_or(Activity::Idle)
}

/// `PiglinAi.isIdle(piglin)`: idle is active.
pub(crate) fn is_idle(mob: &Mob) -> bool {
    recorded_activity(mob) == Activity::Idle
}

/// The record of the mob `key` as the view has it, decoded, with its `lived`.
fn mob_record(cx: &Cx<'_, '_, '_>, key: EntityId) -> Option<(u16, Mob, u32, Vec3)> {
    let seen = cx.ai.tick.world.entity(key)?;
    let mob = Mob::decode(&seen.entity.data).ok()?;
    Some((
        seen.entity.kind,
        mob,
        seen.entity.lived,
        seen.entity.position,
    ))
}

/// `Hoglin.canBeHunted()` of a hoglin the view has: its type's
/// [`super::MobType::cannot_be_hunted`], negated.
fn hoglin_can_be_hunted(cx: &Cx<'_, '_, '_>, key: EntityId) -> bool {
    mob_record(cx, key).is_some_and(|(kind, mob, _, _)| !super::of(kind).cannot_be_hunted(&mob))
}

/// The memories of another piglin (or brute) of the view, from its record: `None` for
/// another type, or one only a copy shows (whose record keeps no mind).
fn memories_of_piglin(cx: &Cx<'_, '_, '_>, key: EntityId) -> Option<Memories> {
    let (kind, mob, lived, _) = mob_record(cx, key)?;
    let provider = match kind {
        PIGLIN => &PROVIDER,
        PIGLIN_BRUTE => &super::piglin_brute::PROVIDER,
        _ => return None,
    };
    Some(brain::memories_of(provider, &mob, lived))
}

/// `wasHurtRecently(entity)` of the mob itself: its `hurt_by`.
fn hurt_recently(memories: &Memories) -> bool {
    memories.check(MemoryKind::HurtBy, Status::Present)
}

/// `hoglinsOutnumberPiglins(piglin)`: more hoglins than piglins and it, as it saw them.
fn hoglins_outnumber_piglins(memories: &Memories) -> bool {
    let piglins = memories
        .int(MemoryKind::VisibleAdultPiglinCount)
        .unwrap_or(0)
        + 1;
    let hoglins = memories
        .int(MemoryKind::VisibleAdultHoglinCount)
        .unwrap_or(0);
    hoglins > piglins
}

/// A list of entities a memory holds, none where it holds none.
fn list_of(memories: &Memories, kind: MemoryKind) -> Vec<EntityId> {
    match memories.get(kind) {
        Some(Value::Entities(list)) => list,
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------------------------------
// Anger.

/// `dontKillAnyMoreHoglinsForAWhile(piglin)`: it has hunted recently, for 30 to 120
/// seconds of the level's draws.
fn dont_hunt_for_a_while(cx: &mut Cx<'_, '_, '_>) {
    let ttl = sample(cx.ai.tick.draws, HUNTS);
    cx.memories.set_with_expiry(
        MemoryKind::HuntedRecently,
        Value::Bool(true),
        i64::from(ttl),
    );
}

/// `PiglinAi.setAngerTarget(level, piglin, target)` (1337–1382): where it may attack
/// the target whatever is in the way, angry at its UUID for 600 ticks, the unreachable
/// walk forgotten; a hoglin, where it may hunt, not hunted again for a while. Universal
/// anger is off.
pub(crate) fn set_anger_target(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    if !attackable_ignoring_line_of_sight(cx, target) {
        return;
    }
    let Some(uuid) = uuid_of(cx, target.key) else {
        return;
    };
    cx.memories.erase(MemoryKind::CantReachWalkTargetSince);
    cx.memories
        .set_with_expiry(MemoryKind::AngryAt, Value::Uuid(uuid), ANGER_DURATION);
    if target.kind == Some(HOGLIN) && can_hunt(cx) {
        dont_hunt_for_a_while(cx);
    }
}

/// `BehaviorUtils.getNearestTarget(mob, current, other)`: the other where there is no
/// current, else the nearer of the two to the mob, the other on a tie.
fn nearest_target(cx: &Cx<'_, '_, '_>, current: Option<Looked>, other: &Looked) -> Looked {
    let from = cx.ai.position();
    match current {
        Some(current) if current.distance_sqr(from) < other.distance_sqr(from) => current,
        _ => other.clone(),
    }
}

/// `setAngerTargetIfCloserThanCurrent(level, piglin, target)` (1403–1424).
fn set_anger_target_if_closer(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    let current = match cx.memories.get(MemoryKind::AngryAt) {
        Some(Value::Uuid(uuid)) => living_of_uuid(cx, uuid),
        _ => None,
    };
    let current_key = current.as_ref().map(|looked| looked.key);
    let nearest = nearest_target(cx, current, target);
    if current_key == Some(nearest.key) {
        return;
    }
    set_anger_target(cx, &nearest);
}

/// The telling of the thing `target` as `told`, from its record or face.
fn telling(cx: &Cx<'_, '_, '_>, what: ToldWhat, target: &Looked) -> Told {
    let player = target
        .is_player()
        .then(|| uuid_of(cx, target.key))
        .flatten()
        .map(uuid::Uuid::from_u128);
    Told {
        what,
        of: target.key,
        kind: target.kind.unwrap_or(PLAYER),
        player,
        eyes: [
            Double(target.position[0]),
            Double(target.eye_y),
            Double(target.position[2]),
        ],
    }
}

/// `broadcastAngerTarget(level, piglin, target)` (1320–1328): each adult piglin and
/// brute it lives near (`NEARBY_ADULT_PIGLINS`) is told of the target, which takes it
/// where it would (`told`, kept for its next step).
pub(crate) fn broadcast_anger_target(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    let told = telling(cx, ToldWhat::TargetOf, target);
    for key in list_of(cx.memories, MemoryKind::NearbyAdultPiglins) {
        cx.ai.tick.out.local.push(Local::Tell {
            key,
            told: Box::new(told.clone()),
        });
    }
}

/// `maybeRetaliate(level, piglin, attacker)` (1147–1191): not while avoiding, nor at
/// what it may not attack or that is much further than its present target (by 4);
/// else angry at the attacker, and its anger told to those near.
pub(crate) fn maybe_retaliate(cx: &mut Cx<'_, '_, '_>, avoiding: bool, attacker: &Looked) {
    if avoiding || !attackable_ignoring_line_of_sight(cx, attacker) {
        return;
    }
    if let Some(current) = entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
    {
        let from = cx.ai.position();
        if attacker.distance_sqr(from) > current.distance_sqr(from) + 4.0 * 4.0 {
            return;
        }
    }
    // Universal anger is off: the attacker itself.
    set_anger_target(cx, attacker);
    broadcast_anger_target(cx, attacker);
}

/// `findNearestValidAttackTarget(level, piglin)` (944–1004): none near a zombified one;
/// what it is angry at, where it may attack it whatever is in the way; with universal
/// anger the nearest player it may attack; its nemesis; the nearest player not in gold
/// it may attack.
fn nearest_valid_attack_target(cx: &mut Cx<'_, '_, '_>) -> Option<EntityId> {
    if is_near_zombified(cx) {
        return None;
    }
    if let Some(Value::Uuid(uuid)) = cx.memories.get(MemoryKind::AngryAt)
        && let Some(angry) = living_of_uuid(cx, uuid)
        && attackable_ignoring_line_of_sight(cx, &angry)
    {
        return Some(angry.key);
    }
    if cx
        .memories
        .check(MemoryKind::UniversalAnger, Status::Present)
        && let Some(player) = entity_of(cx, MemoryKind::NearestVisibleTargetablePlayer)
    {
        return Some(player);
    }
    if let Some(nemesis) = entity_of(cx, MemoryKind::NearestVisibleNemesis) {
        return Some(nemesis);
    }
    if let Some(player) = entity_of(cx, MemoryKind::NearestTargetablePlayerNotWearingGold)
        .and_then(|key| find(cx.ai, key))
        && attackable(cx, &player)
    {
        return Some(player.key);
    }
    None
}

fn not_nearest_valid_attack_target(cx: &mut Cx<'_, '_, '_>, target: &Looked) -> bool {
    nearest_valid_attack_target(cx) != Some(target.key)
}

/// `setAvoidTargetAndDontHuntForAWhile(piglin, target)` (1565–1591).
fn set_avoid_target(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    cx.memories.erase(MemoryKind::AngryAt);
    cx.memories.erase(MemoryKind::AttackTarget);
    cx.memories.erase(MemoryKind::WalkTarget);
    let ttl = sample(cx.ai.tick.draws, RETREAT_DURATION);
    cx.memories.set_with_expiry(
        MemoryKind::AvoidTarget,
        Value::Entity(target.key),
        i64::from(ttl),
    );
    dont_hunt_for_a_while(cx);
}

/// `broadcastRetreat(piglin, target)` (1457–1489): each piglin it sees
/// (`NEAREST_VISIBLE_ADULT_PIGLINS`) avoids the nearest to it of what it avoids, what it
/// attacks and the target (`retreatFromNearestTarget`), and does not hunt for a while:
/// written into the region's own (named in the module's head), with this piglin's draws
/// on the level's behalf.
fn broadcast_retreat(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    for key in list_of(cx.memories, MemoryKind::NearestVisibleAdultPiglins) {
        let Some((PIGLIN, _, _, at)) = mob_record(cx, key) else {
            continue;
        };
        let Some(theirs) = memories_of_piglin(cx, key) else {
            continue;
        };
        let nearest_to =
            |cx: &Cx<'_, '_, '_>, current: Option<EntityId>, other: Looked| match current
                .and_then(|key| find(cx.ai, key))
            {
                Some(current) if current.distance_sqr(at) < other.distance_sqr(at) => current,
                _ => other,
            };
        let avoided = match theirs.get(MemoryKind::AvoidTarget) {
            Some(Value::Entity(key)) => Some(key),
            _ => None,
        };
        let attacked = match theirs.get(MemoryKind::AttackTarget) {
            Some(Value::Entity(key)) => Some(key),
            _ => None,
        };
        let nearest = nearest_to(cx, avoided, target.clone());
        let nearest = nearest_to(cx, attacked, nearest);
        let retreat = sample(cx.ai.tick.draws, RETREAT_DURATION);
        let hunts = sample(cx.ai.tick.draws, HUNTS);
        cx.ai.tick.out.local.push(Local::SetMemories {
            key,
            writes: vec![
                MemoryWrite::Erase(MemoryKind::AngryAt),
                MemoryWrite::Erase(MemoryKind::AttackTarget),
                MemoryWrite::Erase(MemoryKind::WalkTarget),
                MemoryWrite::SetFor(
                    MemoryKind::AvoidTarget,
                    Value::Entity(nearest.key),
                    i64::from(retreat),
                ),
                MemoryWrite::SetFor(
                    MemoryKind::HuntedRecently,
                    Value::Bool(true),
                    i64::from(hunts),
                ),
            ],
        });
    }
}

/// `PiglinAi.wasHurtBy(level, piglin, attacker)` (1073–1146): nothing for another
/// piglin; what it holds in its off hand let go (no barter), its celebration, dance and
/// admiring ended, admiring disabled for 400 ticks by a player; what it avoids
/// forgotten where it is of another type; a baby avoids the attacker for 100 ticks
/// (and tells of it); outnumbered by hoglins, it retreats from one and tells the others
/// to; else it may retaliate.
fn was_hurt_by(cx: &mut Cx<'_, '_, '_>, other: &Other, attacker: Option<&Looked>) {
    if other.kind == PIGLIN {
        return;
    }
    if off_hand(&cx.ai.tick.mob).is_some() {
        stop_holding_off_hand_item(cx, false);
    }
    cx.memories.erase(MemoryKind::CelebrateLocation);
    cx.memories.erase(MemoryKind::Dancing);
    cx.memories.erase(MemoryKind::AdmiringItem);
    if other.kind == PLAYER {
        cx.memories.set_with_expiry(
            MemoryKind::AdmiringDisabled,
            Value::Bool(true),
            HIT_BY_PLAYER_MEMORY_TIMEOUT,
        );
    }
    if let Some(avoided) = entity_of(cx, MemoryKind::AvoidTarget).and_then(|key| find(cx.ai, key))
        && avoided.kind.unwrap_or(PLAYER) != other.kind
    {
        cx.memories.erase(MemoryKind::AvoidTarget);
    }
    if baby(cx) {
        cx.memories.set_with_expiry(
            MemoryKind::AvoidTarget,
            Value::Entity(other.key),
            BABY_FLEE_DURATION,
        );
        if let Some(attacker) = attacker
            && attackable_ignoring_line_of_sight(cx, attacker)
        {
            broadcast_anger_target(cx, attacker);
        }
        return;
    }
    // What follows reads where the attacker is: one the view has not is "unknown".
    let Some(attacker) = attacker else {
        return;
    };
    if other.kind == HOGLIN && hoglins_outnumber_piglins(cx.memories) {
        set_avoid_target(cx, attacker);
        broadcast_retreat(cx, attacker);
        return;
    }
    let avoiding = recorded_activity(&cx.ai.tick.mob) == Activity::Avoid;
    maybe_retaliate(cx, avoiding, attacker);
}

// ------------------------------------------------------------------------------------
// Holding, throwing and keeping items.

/// `throwItemsTowardPos(piglin, items, pos)`: each thrown towards a block above `pos`
/// (`BehaviorUtils.throwItem`), its off hand's swing first (not shown, see
/// `piglin.rs`).
fn throw_items_toward(cx: &mut Cx<'_, '_, '_>, stacks: Vec<ItemStack>, pos: Vec3) {
    if stacks.is_empty() {
        return;
    }
    let eye_y = cx.ai.eye_y();
    let above = [pos[0], pos[1] + 1.0, pos[2]];
    for stack in stacks {
        piglin::throw_item(cx.ai.tick, eye_y, stack, above);
    }
}

/// `getRandomNearbyPos(piglin)`: `LandRandomPos.getPos(piglin, 4, 2)`, else where it is.
fn random_nearby_pos(cx: &mut Cx<'_, '_, '_>) -> Vec3 {
    random_pos::land_pos(cx.ai, 4, 2).unwrap_or_else(|| cx.ai.position())
}

/// `throwItems(piglin, items)` (703–723): towards the nearest player it sees, else a
/// random place near. A player the view has not is taken as unseen.
fn throw_items(cx: &mut Cx<'_, '_, '_>, stacks: Vec<ItemStack>) {
    let player = entity_of(cx, MemoryKind::NearestVisiblePlayer).and_then(|key| find(cx.ai, key));
    let pos = match player {
        Some(player) => player.position,
        None => random_nearby_pos(cx),
    };
    throw_items_toward(cx, stacks, pos);
}

/// `putInInventory(piglin, stack)` (692–701): into the inventory, what does not fit
/// thrown towards a random place near, which the game draws even where all of it fit.
fn put_in_inventory(cx: &mut Cx<'_, '_, '_>, stack: ItemStack) {
    let mut left = None;
    update(&mut cx.ai.tick.mob, |own| {
        left = piglin::add_to_inventory(own, stack)
    });
    let pos = random_nearby_pos(cx);
    throw_items_toward(cx, left.into_iter().collect(), pos);
}

/// `equipItemIfPossible(level, stack)` in a brain's step: what it replaced falls
/// (`spawnAtLocation`); whether it was worn.
fn equip(cx: &mut Cx<'_, '_, '_>, stack: &ItemStack) -> bool {
    let kind = cx.ai.tick.kind;
    let tick = &mut *cx.ai.tick;
    let equipped = piglin::equip_if_possible(kind, &mut tick.mob, stack, tick.draws);
    if let Some(dropped) = equipped.dropped {
        piglin::spawn_at_location(tick, dropped);
    }
    equipped.worn.is_some()
}

/// `stopHoldingOffHandItem(level, piglin, barter)` (600–671): what the off hand held
/// taken from it; an adult barters a gold ingot (where `barter`), keeps anything else
/// worn or in its inventory, and drops a gold ingot it does not barter; a baby wears it
/// where it can, else holds it in its main hand, what was there kept (a loved item) or
/// thrown.
pub(crate) fn stop_holding_off_hand_item(cx: &mut Cx<'_, '_, '_>, barter: bool) {
    let Some(held) = cx.ai.tick.mob.body.equipment.slots[usize::from(slots::OFF_HAND)].take()
    else {
        return;
    };
    if !baby(cx) {
        let currency = is_barter_currency(&held);
        if barter && currency {
            let given = super::piglin_barter::barter(cx.ai.tick.draws);
            throw_items(cx, given);
        } else if !currency && !equip(cx, &held) {
            put_in_inventory(cx, held);
        }
        return;
    }
    if !equip(cx, &held) {
        let main = cx.ai.tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
            .clone()
            .filter(|stack| stack.count > 0);
        match main {
            Some(main) if is_loved(&main) => put_in_inventory(cx, main),
            main => throw_items(cx, main.into_iter().collect()),
        }
        hold_in_main_hand(&mut cx.ai.tick.mob, held);
    }
}

/// `PiglinAi.holdInOffhand(level, piglin, stack)`: what its off hand held falls, and it
/// holds the stack there.
fn hold_in_offhand(tick: &mut MobTick<'_>, stack: ItemStack) {
    if let Some(held) = off_hand(&tick.mob).cloned() {
        piglin::spawn_at_location(tick, held);
    }
    hold_in_off_hand(&mut tick.mob, stack);
}

/// `cancelAdmiring(level, piglin)`: admiring, what it holds in its off hand falls.
pub(crate) fn cancel_admiring(tick: &mut MobTick<'_>) {
    let memories = brain::memories_of(&PROVIDER, &tick.mob, tick.lived.saturating_add(1));
    if !admiring(&memories) {
        return;
    }
    if let Some(held) = tick.mob.body.equipment.slots[usize::from(slots::OFF_HAND)].take() {
        piglin::spawn_at_location(tick, held);
    }
}

/// `admireGoldItem(piglin)`: it admires for 119 ticks.
fn admire(memories: &mut Memories) {
    memories.set_with_expiry(MemoryKind::AdmiringItem, Value::Bool(true), ADMIRE_DURATION);
}

/// `canAdmire(piglin, stack)`: an adult, not admiring nor disabled, given a gold ingot.
fn can_admire(mob: &Mob, memories: &Memories, stack: &ItemStack) -> bool {
    !admiring_disabled(memories)
        && !admiring(memories)
        && !piglin::is_baby(mob)
        && is_barter_currency(stack)
}

/// `PiglinAi.mobInteract(level, piglin, player, hand)` (1029–1054): a gold ingot it may
/// admire taken (one of it; a creative player keeps theirs), held in its off hand and
/// admired, and it stops walking. Its previous off hand's item falls at its feet.
pub(crate) fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let Some(held) = used.held.clone().filter(|stack| stack.count > 0) else {
        return Interaction::Pass;
    };
    let mut memories = brain::memories_of(&PROVIDER, used.mob, used.moment);
    if !can_admire(used.mob, &memories, &held) {
        return Interaction::Pass;
    }
    if let Some(previous) = off_hand(used.mob).cloned() {
        used.did.push(Did::Drop {
            stack: previous,
            at: used.position,
            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
            push: None,
        });
    }
    hold_in_off_hand(used.mob, held.with_count(1));
    admire(&mut memories);
    // `stopWalking`: no walk target, and no path.
    memories.erase(MemoryKind::WalkTarget);
    brain::write_memories(&PROVIDER, used.mob, memories);
    used.mob.body.path.followed = None;
    Interaction::Success
}

/// `PiglinAi.wantsToPickup(piglin, stack)` (812–875): what a piglin wants of an item it
/// sees or touches.
pub(crate) fn wants_to_pickup(mob: &Mob, memories: &Memories, stack: &ItemStack) -> bool {
    let own = read(mob);
    if own.baby && item_in_tag(stack, "minecraft:ignored_by_piglin_babies") {
        return false;
    }
    if item_in_tag(stack, "minecraft:piglin_repellents") {
        return false;
    }
    if admiring_disabled(memories) && memories.check(MemoryKind::AttackTarget, Status::Present) {
        return false;
    }
    if is_barter_currency(stack) {
        return not_holding_loved_in_off_hand(mob);
    }
    let room = piglin::can_add_to_inventory(&own, stack);
    if stack.item == items::GOLD_NUGGET {
        return room;
    }
    if is_food(stack) {
        return !ate_recently(memories) && room;
    }
    if is_loved(stack) {
        return not_holding_loved_in_off_hand(mob) && room;
    }
    piglin::can_replace_current(PIGLIN, mob, stack)
}

fn item_in_tag(stack: &ItemStack, tag: &str) -> bool {
    clustine_blocks::data::item_in_tag(stack.item, tag)
}

/// `Piglin.wantsToPickUp(level, stack)`: `mob_griefing` (on), `canPickUpLoot`, and
/// `PiglinAi`'s; a brute's wants a golden axe (`PiglinBrute.wantsToPickUp`, `Mob`'s
/// `canHoldItem`).
pub(crate) fn wants_to_pick_up(
    kind: u16,
    mob: &Mob,
    memories: &Memories,
    stack: &ItemStack,
) -> bool {
    if !mob.body.flags.picks_up_loot {
        return false;
    }
    match kind {
        PIGLIN => wants_to_pickup(mob, memories, stack),
        _ => stack.item == items::GOLDEN_AXE,
    }
}

/// `PiglinAi.pickUpItem(level, piglin, item)` (499–563): it stops walking; it takes a
/// stack of gold nuggets whole, else one of the stack; a loved item it holds in its off
/// hand and admires; food it eats where it has not lately; else it wears what it can,
/// and keeps the rest in its inventory.
fn pick_up_item(cx: &mut Cx<'_, '_, '_>, item: EntityId, stack: &ItemStack) {
    cx.memories.erase(MemoryKind::WalkTarget);
    cx.ai.tick.mob.body.path.followed = None;
    let taken = if stack.item == items::GOLD_NUGGET {
        cx.ai.tick.out.local.push(Local::TakeItem {
            item,
            count: stack.count,
        });
        stack.clone()
    } else {
        cx.ai
            .tick
            .out
            .local
            .push(Local::TakeSome { item, count: 1 });
        stack.with_count(1)
    };
    if is_loved(&taken) {
        cx.memories.erase(MemoryKind::TimeTryingToReachAdmireItem);
        hold_in_offhand(cx.ai.tick, taken);
        admire(cx.memories);
        return;
    }
    if is_food(&taken) && !ate_recently(cx.memories) {
        cx.memories
            .set_with_expiry(MemoryKind::AteRecently, Value::Bool(true), EAT_COOLDOWN);
        return;
    }
    if equip(cx, &taken) {
        return;
    }
    put_in_inventory(cx, taken);
}

/// The memories of the mob of `tick` with its region's sensed ones, at `now`, its AI step
/// and the game time, for `f`; its memories written back after. Nothing for a type with
/// no brain.
pub(crate) fn with_cx<R>(
    tick: &mut MobTick<'_>,
    now: u32,
    f: impl FnOnce(&mut Cx<'_, '_, '_>) -> R,
) -> Option<R> {
    let provider = tick.mob_type.brain()?;
    let brain = Brain::read(provider, &tick.mob.mind.brain, tick.sensed.as_ref(), now);
    let mut memories = brain.memories;
    let mut setup = Setup::default();
    tick.mob_type.register_goals(&mut setup);
    let config = setup.config;
    let time = tick.world.game_time();
    let result = {
        let mut ai = Ai {
            tick,
            config: &config,
            sensing: sensing::Sensing::default(),
            navigation_ticked: true,
        };
        let mut cx = Cx {
            ai: &mut ai,
            memories: &mut memories,
            time,
        };
        f(&mut cx)
    };
    brain::write_memories(provider, &mut tick.mob, memories);
    Some(result)
}

/// `Mob.aiStep`'s pick-up after its brain's tick (n7 Mob.txt 884–960): alive, picking
/// up loot (`mob_griefing` is on), each item of the region's within its box grown by a
/// block sideways, with no pick-up delay, that it wants, in turn (a piglin's
/// `PiglinAi.pickUpItem`, a brute's `Mob.pickUpItem`).
pub(crate) fn pick_up(tick: &mut MobTick<'_>) {
    if !tick.mob.body.flags.picks_up_loot || tick.mob.body.vitals.dying() {
        return;
    }
    let body = &tick.living.body;
    let bounds = crate::physics::aabb::Aabb::of_entity(body.position, body.width, body.height)
        .inflate([1.0, 0.0, 1.0]);
    let found: Vec<(EntityId, ItemStack)> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                ref stack,
                pick_up_delay: 0,
                ..
            } if stack.count > 0 => Some((seen.entity.key, stack.clone())),
            _ => None,
        })
        .collect();
    if found.is_empty() {
        return;
    }
    let now = tick.lived;
    with_cx(tick, now, |cx| {
        for (key, stack) in found {
            let kind = cx.ai.tick.kind;
            if !wants_to_pick_up(kind, &cx.ai.tick.mob, cx.memories, &stack) {
                continue;
            }
            if kind == PIGLIN {
                pick_up_item(cx, key, &stack);
            } else {
                // `Mob.pickUpItem`: what it wears of the stack is taken.
                let tick = &mut *cx.ai.tick;
                let equipped = piglin::equip_if_possible(kind, &mut tick.mob, &stack, tick.draws);
                if let Some(dropped) = equipped.dropped {
                    piglin::spawn_at_location(tick, dropped);
                }
                if let Some(worn) = equipped.worn {
                    tick.out.local.push(Local::TakeSome {
                        item: key,
                        count: worn.count,
                    });
                }
            }
        }
    });
}

/// What a piglin or a brute takes of being told (`MobType::told`): a player's anger
/// where it is idle and, where the telling asks, sees them (`angerNearbyPiglins`'s two
/// filters); another's target always. Kept for its next step.
pub(crate) fn told(mob: &mut Mob, told: &Told, seen: bool) -> bool {
    let other = piglin::other(told.of, told.kind, told.player.map(|uuid| uuid.as_u128()));
    let pending = match told.what {
        ToldWhat::AngerAt { only_if_seen } => {
            if (only_if_seen && !seen) || !is_idle(mob) {
                return false;
            }
            Pending::Anger(other)
        }
        ToldWhat::TargetOf => Pending::Target(other),
    };
    update(mob, |own| own.pending.push(pending));
    true
}

/// What was told or hurt the piglin or brute of `tick` since its last step, taken in
/// order before its brain's tick, as of its last (see the module's head).
pub(crate) fn take_pending(tick: &mut MobTick<'_>) {
    if read(&tick.mob).pending.is_empty() {
        return;
    }
    let mut pending = Vec::new();
    update(&mut tick.mob, |own| {
        pending = std::mem::take(&mut own.pending)
    });
    let now = tick.lived.saturating_sub(1);
    with_cx(tick, now, |cx| {
        for noted in pending {
            take_one(cx, noted);
        }
    });
}

/// One thing told or hurt by, of a piglin or a brute. Of an entity its view has not
/// ("unknown") only what needs no sight of it is done: a piglin's hurt ends its
/// admiring and its celebration as any does.
fn take_one(cx: &mut Cx<'_, '_, '_>, noted: Pending) {
    let (Pending::Hurt(other) | Pending::Anger(other) | Pending::Target(other)) = noted.clone();
    let target = find(cx.ai, other.key);
    let brute = cx.ai.tick.kind == PIGLIN_BRUTE;
    match noted {
        Pending::Hurt(_) if brute => {
            if let Some(target) = target {
                super::piglin_brute::was_hurt_by(cx, &target);
            }
        }
        Pending::Hurt(_) => was_hurt_by(cx, &other, target.as_ref()),
        Pending::Anger(_) => {
            if let Some(target) = target {
                set_anger_target(cx, &target);
            }
        }
        Pending::Target(Other { kind, key, .. }) => {
            // `broadcastAngerTarget`'s receiver: a hoglin only where it may hunt and the
            // hoglin may be hunted.
            let Some(target) = target else {
                return;
            };
            if kind == HOGLIN && (!can_hunt(cx) || !hoglin_can_be_hunted(cx, key)) {
                return;
            }
            set_anger_target_if_closer(cx, &target);
        }
    }
}

// ------------------------------------------------------------------------------------
// Sounds.

/// `isNearAvoidTarget(piglin)`: what it avoids within 12. One the view has not is taken
/// as far.
fn near_avoid_target(cx: &mut Cx<'_, '_, '_>) -> bool {
    entity_of(cx, MemoryKind::AvoidTarget)
        .and_then(|key| find(cx.ai, key))
        .is_some_and(|avoided| closer_than(cx.ai, &avoided, AVOIDING_DISTANCE))
}

/// `getSoundForActivity(piglin, activity)` (1201–1234).
fn sound_for(cx: &mut Cx<'_, '_, '_>, activity: Activity) -> i32 {
    if activity == Activity::Fight {
        return sounds::ENTITY_PIGLIN_ANGRY;
    }
    if piglin::is_converting(cx.ai.tick) {
        return sounds::ENTITY_PIGLIN_RETREAT;
    }
    if activity == Activity::Avoid && near_avoid_target(cx) {
        return sounds::ENTITY_PIGLIN_RETREAT;
    }
    if activity == Activity::AdmireItem {
        return sounds::ENTITY_PIGLIN_ADMIRING_ITEM;
    }
    if activity == Activity::Celebrate {
        return sounds::ENTITY_PIGLIN_CELEBRATE;
    }
    if sees_player_holding_loved(cx.memories) {
        return sounds::ENTITY_PIGLIN_JEALOUS;
    }
    if cx
        .memories
        .check(MemoryKind::NearestRepellent, Status::Present)
    {
        return sounds::ENTITY_PIGLIN_RETREAT;
    }
    sounds::ENTITY_PIGLIN_AMBIENT
}

/// `getAmbientSound()` of an idle piglin (`AbstractPiglin.playAmbientSound` plays only
/// then): idle's sound, as of its last tick.
pub(crate) fn idle_sound(tick: &mut MobTick<'_>) -> i32 {
    let now = tick.lived.saturating_sub(1);
    with_cx(tick, now, |cx| sound_for(cx, Activity::Idle)).unwrap_or(sounds::ENTITY_PIGLIN_AMBIENT)
}

/// `PiglinAi.updateActivity(piglin)` (406–466): the first of admiring, fighting,
/// avoiding, celebrating, riding and idling whose memories it has; a change heard
/// (`makeSound` of the new activity's sound); aggressive while it has an attack target;
/// the dance over with the celebration.
pub(crate) fn update_activity(brain: &mut Brain, ai: &mut Ai<'_, '_>) {
    let before = brain.active_non_core();
    brain.set_active_to_first_valid(&[
        Activity::AdmireItem,
        Activity::Fight,
        Activity::Avoid,
        Activity::Celebrate,
        Activity::Ride,
        Activity::Idle,
    ]);
    let after = brain.active_non_core();
    if before != after
        && let Some(activity) = after
    {
        let time = ai.tick.world.game_time();
        let sound = {
            let mut cx = Cx {
                ai: &mut *ai,
                memories: &mut brain.memories,
                time,
            };
            sound_for(&mut cx, activity)
        };
        let volume = ai.tick.mob_type.sound_volume_of(&ai.tick.mob);
        body::make_sound(ai.tick, Some(sound), volume);
    }
    ai.tick.mob.body.combat.aggressive = brain
        .memories
        .check(MemoryKind::AttackTarget, Status::Present);
    if !brain
        .memories
        .check(MemoryKind::CelebrateLocation, Status::Present)
    {
        brain.memories.erase(MemoryKind::Dancing);
    }
    let dancing = brain.memories.check(MemoryKind::Dancing, Status::Present);
    update(&mut ai.tick.mob, |own| own.dancing = dancing);
}

// ------------------------------------------------------------------------------------
// The piglin's own behaviours.

/// `StopHoldingItemIfNoLongerAdmiring.create()`: no longer admiring, what it holds in its
/// off hand (but a shield, `BLOCKS_ATTACKS`) let go, bartered where it is a gold ingot.
struct StopHoldingItemIfNoLongerAdmiring;

impl Shot for StopHoldingItemIfNoLongerAdmiring {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::AdmiringItem]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::AdmiringItem, Status::Absent) {
            return false;
        }
        let Some(held) = off_hand(&cx.ai.tick.mob) else {
            return false;
        };
        if held.item == items::SHIELD {
            return false;
        }
        stop_holding_off_hand_item(cx, true);
        true
    }
}

/// The stack of the item entity `key` as the view has it.
fn item_stack(cx: &Cx<'_, '_, '_>, key: EntityId) -> Option<ItemStack> {
    match &cx.ai.tick.world.entity(key)?.entity.data {
        EntityData::Item { stack, .. } => Some(stack.clone()),
        _ => None,
    }
}

/// `StartAdmiringItemIfSeen.create(119)`: a loved item it sees and wants, while it may
/// admire and walk to admire, admired for 119 ticks.
struct StartAdmiringItemIfSeen;

impl Shot for StartAdmiringItemIfSeen {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::NearestVisibleWantedItem,
            MemoryKind::AdmiringItem,
            MemoryKind::AdmiringDisabled,
            MemoryKind::DisableWalkToAdmireItem,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::NearestVisibleWantedItem, Status::Present)
            || !cx.memories.check(MemoryKind::AdmiringItem, Status::Absent)
            || !cx
                .memories
                .check(MemoryKind::AdmiringDisabled, Status::Absent)
            || !cx
                .memories
                .check(MemoryKind::DisableWalkToAdmireItem, Status::Absent)
        {
            return false;
        }
        let Some(stack) =
            entity_of(cx, MemoryKind::NearestVisibleWantedItem).and_then(|key| item_stack(cx, key))
        else {
            return false;
        };
        if !is_loved(&stack) {
            return false;
        }
        admire(cx.memories);
        true
    }
}

/// `StopAdmiringIfItemTooFarAway.create(9)`: with nothing in its off hand, admiring ends
/// where the wanted item is gone or 9 or more away. One the view has not is left be.
struct StopAdmiringIfItemTooFarAway;

impl Shot for StopAdmiringIfItemTooFarAway {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::AdmiringItem,
            MemoryKind::NearestVisibleWantedItem,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::AdmiringItem, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::NearestVisibleWantedItem, Status::Registered)
        {
            return false;
        }
        if off_hand(&cx.ai.tick.mob).is_some() {
            return false;
        }
        if let Some(key) = entity_of(cx, MemoryKind::NearestVisibleWantedItem) {
            match find(cx.ai, key) {
                None => return false,
                Some(item) if closer_than(cx.ai, &item, f64::from(ITEM_REACH)) => return false,
                Some(_) => {}
            }
        }
        cx.memories.erase(MemoryKind::AdmiringItem);
        true
    }
}

/// `StopAdmiringIfTiredOfTryingToReachItem.create(200, 200)`: with nothing in its off
/// hand, each tick it admires an item it sees counts; past 200, admiring ends and its
/// walk to admire is disabled for 200 ticks.
struct StopAdmiringIfTiredOfTryingToReachItem;

impl Shot for StopAdmiringIfTiredOfTryingToReachItem {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::AdmiringItem,
            MemoryKind::NearestVisibleWantedItem,
            MemoryKind::TimeTryingToReachAdmireItem,
            MemoryKind::DisableWalkToAdmireItem,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::AdmiringItem, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::NearestVisibleWantedItem, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::TimeTryingToReachAdmireItem, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::DisableWalkToAdmireItem, Status::Registered)
        {
            return false;
        }
        if off_hand(&cx.ai.tick.mob).is_some() {
            return false;
        }
        match cx.memories.int(MemoryKind::TimeTryingToReachAdmireItem) {
            None => cx
                .memories
                .set(MemoryKind::TimeTryingToReachAdmireItem, Value::Int(0)),
            Some(time) if time > ITEM_WALK_TIME => {
                cx.memories.erase(MemoryKind::AdmiringItem);
                cx.memories.erase(MemoryKind::TimeTryingToReachAdmireItem);
                cx.memories.set_with_expiry(
                    MemoryKind::DisableWalkToAdmireItem,
                    Value::Bool(true),
                    ITEM_WALK_DISABLED,
                );
            }
            Some(time) => cx.memories.set(
                MemoryKind::TimeTryingToReachAdmireItem,
                Value::Int(time + 1),
            ),
        }
        true
    }
}

/// `StartHuntingHoglin.create()`: an adult that sees a hoglin it may hunt, angry at
/// nothing, and that neither it nor an adult piglin it sees has hunted lately, is angry
/// at it, tells those near, and none of them hunts again for a while.
struct StartHuntingHoglin;

impl Shot for StartHuntingHoglin {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::NearestVisibleHuntableHoglin,
            MemoryKind::AngryAt,
            MemoryKind::HuntedRecently,
            MemoryKind::NearestVisibleAdultPiglins,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::NearestVisibleHuntableHoglin, Status::Present)
            || !cx.memories.check(MemoryKind::AngryAt, Status::Absent)
            || !cx
                .memories
                .check(MemoryKind::HuntedRecently, Status::Absent)
            || !cx
                .memories
                .check(MemoryKind::NearestVisibleAdultPiglins, Status::Registered)
        {
            return false;
        }
        let others = list_of(cx.memories, MemoryKind::NearestVisibleAdultPiglins);
        let hunted = others.iter().any(|key| {
            memories_of_piglin(cx, *key)
                .is_some_and(|theirs| theirs.check(MemoryKind::HuntedRecently, Status::Present))
        });
        if baby(cx) || hunted {
            return false;
        }
        let Some(hoglin) = entity_of(cx, MemoryKind::NearestVisibleHuntableHoglin)
            .and_then(|key| find(cx.ai, key))
        else {
            return false;
        };
        set_anger_target(cx, &hoglin);
        dont_hunt_for_a_while(cx);
        broadcast_anger_target(cx, &hoglin);
        for key in others {
            let ttl = sample(cx.ai.tick.draws, HUNTS);
            cx.ai.tick.out.local.push(Local::SetMemories {
                key,
                writes: vec![MemoryWrite::SetFor(
                    MemoryKind::HuntedRecently,
                    Value::Bool(true),
                    i64::from(ttl),
                )],
            });
        }
        true
    }
}

/// `RememberIfHoglinWasKilled.create()`: an attack target that is a dead or dying hoglin
/// means it has hunted, for a while.
struct RememberIfHoglinWasKilled;

impl Shot for RememberIfHoglinWasKilled {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::AttackTarget, MemoryKind::HuntedRecently]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::AttackTarget, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::HuntedRecently, Status::Registered)
        {
            return false;
        }
        if let Some(target) =
            entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
            && target.kind == Some(HOGLIN)
            && !target.alive
        {
            dont_hunt_for_a_while(cx);
        }
        true
    }
}

/// `babySometimesRideBabyHoglin()`: `CopyMemoryWithExpiry` of the baby hoglin it sees
/// into its ride target for 10 to 30 seconds, by a baby whose ticker
/// (`SetEntityLookTargetSometimes.Ticker` of 10 to 40 seconds) says.
struct BabyRidesBabyHoglin;

impl KeptShot for BabyRidesBabyHoglin {
    type State = Ticker;

    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::NearestVisibleBabyHoglin, MemoryKind::RideTarget]
    }

    fn trigger(&self, state: &mut Ticker, cx: &mut Cx<'_, '_, '_>) -> bool {
        let now = cx.ai.tick.lived;
        if !cx
            .memories
            .check(MemoryKind::NearestVisibleBabyHoglin, Status::Present)
            || !cx.memories.check(MemoryKind::RideTarget, Status::Absent)
            || !baby(cx)
        {
            state.ticks.pause(now);
            return false;
        }
        // `Ticker.tickDownAndCheck(level.getRandom())`.
        let left = state.ticks.at(now);
        if left == 0 {
            let next = sample(cx.ai.tick.draws, RIDE_START_INTERVAL) - 1;
            state.ticks.set(next, now, true);
            return false;
        }
        if left > 1 {
            state.ticks.lower(now);
            return false;
        }
        state.ticks = Counter::default();
        let Some(value) = cx.memories.get(MemoryKind::NearestVisibleBabyHoglin) else {
            return false;
        };
        let ttl = sample(cx.ai.tick.draws, RIDE_DURATION);
        cx.memories
            .set_with_expiry(MemoryKind::RideTarget, value, i64::from(ttl));
        true
    }

    fn left(&self, state: &mut Ticker, now: u32, _: i64) {
        state.ticks.pause(now + 1);
    }
}

/// `wantsToDance(piglin, target)`: a hoglin killed, one time in ten of a random seeded
/// with the game time (`RandomSource.createThreadLocalInstance(gameTime).nextFloat()`,
/// the JDK's generator).
fn wants_to_dance(cx: &mut Cx<'_, '_, '_>, target: &Looked) -> bool {
    if target.kind != Some(HOGLIN) {
        return false;
    }
    let seed = (cx.time as u64 ^ 0x5_DEEC_E66D) & ((1 << 48) - 1);
    let next = (seed.wrapping_mul(0x5_DEEC_E66D).wrapping_add(0xB)) & ((1 << 48) - 1);
    let float = (next >> (48 - 24)) as f32 / (1 << 24) as f32;
    float < 0.1
}

/// `wantsToStopFleeing(piglin)` (1490–1524): nothing to avoid; a hoglin, where the
/// piglins are no fewer; a zombified one it no longer sees nearest.
fn wants_to_stop_fleeing(cx: &mut Cx<'_, '_, '_>) -> bool {
    let Some(key) = entity_of(cx, MemoryKind::AvoidTarget) else {
        return true;
    };
    let Some(avoided) = find(cx.ai, key) else {
        return false;
    };
    if avoided.kind == Some(HOGLIN) {
        return !hoglins_outnumber_piglins(cx.memories);
    }
    if is_zombified(&avoided) {
        return !cx
            .memories
            .is(MemoryKind::NearestVisibleZombified, &Value::Entity(key));
    }
    false
}

/// `wantsToStopRiding(piglin, entity)` (882–911): of a mob, one grown, dead, hurt lately
/// (the piglin's own hurt; another's is not read, see the module's head), or a piglin
/// that rides nothing.
fn wants_to_stop_riding(cx: &mut Cx<'_, '_, '_>, other: &Looked) -> bool {
    if !other.mob {
        return false;
    }
    !is_baby(cx.ai, other)
        || !other.alive
        || hurt_recently(cx.memories)
        || (other.kind == Some(PIGLIN) && other.vehicle.is_none())
}

fn no_callback(_: &mut Cx<'_, '_, '_>, _: EntityId) {}

/// `MeleeAttack.create(20)` with `isHoldingUsableNonMeleeWeapon`'s refusal of a piglin
/// holding a crossbow (`Piglin.canUseNonMeleeWeapon`).
fn not_holding_crossbow(cx: &Cx<'_, '_, '_>) -> bool {
    !has_crossbow(&cx.ai.tick.mob)
}

fn hit(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    melee_hit(cx.ai, target);
}

/// `Piglin.performRangedAttack(target, power)`: `performCrossbowAttack(this, 1.6)`, then
/// `onCrossbowAttackPerformed` (its `noActionTime` back to nothing).
fn perform_ranged_attack(ai: &mut Ai<'_, '_>, target: &Looked) {
    ranged_weapons::perform_crossbow_attack(ai, target, 1.6);
    if let Some(since) = &mut ai.tick.mob.body.idle.no_action_since {
        *since = ai.tick.lived + 1;
        ai.tick.mob.body.idle_gained.gained = 0;
    }
}

/// `setChargingCrossbow(charging)`.
fn set_charging(mob: &mut Mob, charging: bool) {
    update(mob, |own| own.charging = charging);
}

fn piglin_can_pick_up(cx: &Cx<'_, '_, '_>) -> bool {
    cx.ai.tick.mob.body.flags.picks_up_loot
}

fn not_holding_loved(cx: &Cx<'_, '_, '_>) -> bool {
    not_holding_loved_in_off_hand(&cx.ai.tick.mob)
}

fn timed(min: i64, max: i64) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(DoNothing { min, max }))
}

/// `createLookBehaviors()`: a player, a piglin or anything within 8, alike.
fn look_behaviours() -> Vec<(Box<dyn Behaviour>, u32)> {
    vec![
        (look_at(LookAt::Player, 8.0), 1),
        (look_at(LookAt::Kind(PIGLIN), 8.0), 1),
        (look_at(LookAt::Any, 8.0), 1),
    ]
}

/// `createIdleLookBehaviors()`: the looks, or nothing for 30 to 60 ticks.
fn idle_looks() -> Box<dyn Behaviour> {
    let mut children = look_behaviours();
    children.push((timed(30, 60), 1));
    Box::new(Gate::run_one(children))
}

/// `createIdleMovementBehaviors()`: a stroll, a walk to a piglin, a walk to what it
/// looks at (while it sees no player with a loved item), or nothing.
fn idle_movements() -> Box<dyn Behaviour> {
    Box::new(Gate::run_one(vec![
        (stroll(0.6), 2),
        (
            interact_with(PIGLIN, 8, MemoryKind::InteractionTarget, 0.6, 2),
            2,
        ),
        (
            trigger_if(sees_no_player_holding_loved, walk_to_look_target(0.6, 3)),
            2,
        ),
        (timed(30, 60), 1),
    ]))
}

fn start_attacking() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(StartAttacking {
        condition: adult,
        finder: nearest_valid_attack_target,
    }))
}

fn avoid_repellent() -> Box<dyn Behaviour> {
    away_from_pos(MemoryKind::NearestRepellent, 1.0, 8, false)
}

/// `PiglinAi.getActivities(piglin)` (82–92): core, idle, admiring, fighting, celebrating,
/// retreating, riding.
fn activities() -> Vec<ActivityData> {
    let core = ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            look_sink(45, 90),
            sink(),
            doors::interact_with_door(),
            copy_memory_with_expiry(
                baby_cx,
                MemoryKind::NearestVisibleNemesis,
                MemoryKind::AvoidTarget,
                BABY_AVOID_NEMESIS_DURATION,
            ),
            Box::new(HeldShot::new(CopyMemoryWithExpiry {
                predicate: is_near_zombified,
                memories: [MemoryKind::NearestVisibleZombified, MemoryKind::AvoidTarget],
                duration: AVOID_ZOMBIFIED_DURATION,
            })),
            Box::new(HeldShot::new(StopHoldingItemIfNoLongerAdmiring)),
            Box::new(HeldShot::new(StartAdmiringItemIfSeen)),
            Box::new(HeldShot::new(StartCelebratingIfTargetDead {
                duration: CELEBRATION_TIME,
                dances: wants_to_dance,
            })),
            Box::new(HeldShot::new(StopBeingAngryIfTargetDead)),
        ],
    );
    let idle = ActivityData::numbered(
        Activity::Idle,
        10,
        vec![
            look_at_by(player_holding_loved, 14.0),
            start_attacking(),
            trigger_if(can_hunt_cx, Box::new(HeldShot::new(StartHuntingHoglin))),
            avoid_repellent(),
            Box::new(HeldKept::new(BabyRidesBabyHoglin)),
            idle_looks(),
            idle_movements(),
            set_look_and_interact(PLAYER, 4),
        ],
    );
    let admire = ActivityData::while_present(
        Activity::AdmireItem,
        10,
        vec![
            go_to_wanted_item(GoToWantedItem {
                may: not_holding_loved,
                speed: 1.0,
                override_walk: true,
                range: ITEM_REACH,
                can_pick_up: piglin_can_pick_up,
            }),
            Box::new(HeldShot::new(StopAdmiringIfItemTooFarAway)),
            Box::new(HeldShot::new(StopAdmiringIfTiredOfTryingToReachItem)),
        ],
        MemoryKind::AdmiringItem,
    );
    let fight = ActivityData::while_present(
        Activity::Fight,
        10,
        vec![
            Box::new(HeldShot::new(StopAttackingIfTargetInvalid {
                condition: not_nearest_valid_attack_target,
                callback: no_callback,
                tired: true,
            })),
            trigger_if(
                holds_crossbow,
                Box::new(HeldShot::new(BackUpIfTooClose {
                    distance: 5,
                    speed: 0.75,
                })),
            ),
            Box::new(HeldShot::new(WalkToAttackTarget { speed: 1.0 })),
            // `SpearApproach`, `SpearAttack` and `SpearRetreat` (see the module's head).
            brain::inert(),
            brain::inert(),
            brain::inert(),
            Box::new(HeldShot::new(MeleeAttack {
                predicate: not_holding_crossbow,
                cooldown: 20,
                hit,
            })),
            crossbow_attack(perform_ranged_attack, set_charging),
            Box::new(HeldShot::new(RememberIfHoglinWasKilled)),
            erase_if(is_near_zombified, MemoryKind::AttackTarget),
        ],
        MemoryKind::AttackTarget,
    );
    let celebrate = ActivityData::while_present(
        Activity::Celebrate,
        10,
        vec![
            avoid_repellent(),
            look_at_by(player_holding_loved, 14.0),
            start_attacking(),
            trigger_if(
                not_dancing,
                go_to_target_location(MemoryKind::CelebrateLocation, 2, 1.0),
            ),
            trigger_if(
                dancing,
                go_to_target_location(MemoryKind::CelebrateLocation, 4, 0.6),
            ),
            Box::new(Gate::run_one(vec![
                (look_at(LookAt::Kind(PIGLIN), 8.0), 1),
                (
                    stroll_to(
                        0.6,
                        StrollTo::Land {
                            horizontal: 2,
                            vertical: 1,
                            in_water: true,
                        },
                    ),
                    1,
                ),
                (timed(10, 20), 1),
            ])),
        ],
        MemoryKind::CelebrateLocation,
    );
    let retreat = ActivityData::while_present(
        Activity::Avoid,
        10,
        vec![
            away_from(MemoryKind::AvoidTarget, 1.0, 12, true),
            idle_looks(),
            idle_movements(),
            erase_if(wants_to_stop_fleeing, MemoryKind::AvoidTarget),
        ],
        MemoryKind::AvoidTarget,
    );
    let mut rides = look_behaviours();
    rides.push((Box::new(HeldShot::new(Always)), 1));
    let ride = ActivityData::while_present(
        Activity::Ride,
        10,
        vec![
            Box::new(HeldShot::new(Mount { speed: 0.8 })),
            look_at_by(player_holding_loved, 8.0),
            trigger_if(is_passenger, Box::new(Gate::run_one(rides))),
            Box::new(HeldShot::new(DismountOrSkipMounting {
                distance: 8,
                wants_off: wants_to_stop_riding,
            })),
        ],
        MemoryKind::RideTarget,
    );
    vec![core, idle, admire, fight, celebrate, retreat, ride]
}

// ------------------------------------------------------------------------------------
// The sensors.

/// `NEAREST_ITEMS` of a piglin or a brute: `NearestItemSensor` with its type's
/// `wantsToPickUp`, which reads its memories (`mobs/brain/sensors.rs`'s takes the mob
/// alone).
pub(crate) struct ItemSensor;

pub(crate) static NEAREST_ITEMS: ItemSensor = ItemSensor;

impl Sensor for ItemSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::NearestVisibleWantedItem]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let bounds = cx.ai.tick.living.body.bounds().inflate([32.0, 16.0, 32.0]);
        let from = cx.ai.position();
        let mut found: Vec<(Looked, ItemStack)> = cx
            .ai
            .tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter_map(|seen| match &seen.entity.data {
                EntityData::Item { stack, .. } => Some((Looked::of_seen(seen), stack.clone())),
                _ => None,
            })
            .collect();
        found.sort_by(|a, b| a.0.distance_sqr(from).total_cmp(&b.0.distance_sqr(from)));
        let kind = cx.ai.tick.kind;
        let mut wanted = None;
        for (item, stack) in found {
            if wants_to_pick_up(kind, &cx.ai.tick.mob, cx.memories, &stack)
                && item.distance_sqr(from) < 32.0 * 32.0
                && sensing::has_line_of_sight(cx.ai, &item)
            {
                wanted = Some(item.key);
                break;
            }
        }
        match wanted {
            Some(key) => cx
                .memories
                .set(MemoryKind::NearestVisibleWantedItem, Value::Entity(key)),
            None => cx.memories.erase(MemoryKind::NearestVisibleWantedItem),
        }
    }
}

/// `PiglinAi.findNearbyAdultPiglins(brain)`: the adult piglins and brutes of what it
/// lives near (`NEAREST_LIVING_ENTITIES`), nearest first.
pub(crate) fn nearby_adult_piglins(cx: &Cx<'_, '_, '_>) -> Vec<EntityId> {
    list_of(cx.memories, MemoryKind::Mobs)
        .into_iter()
        .filter(|key| {
            mob_record(cx, *key).is_some_and(|(kind, mob, _, _)| {
                kind == PIGLIN_BRUTE || (kind == PIGLIN && !piglin::is_baby(&mob))
            })
        })
        .collect()
}

/// A memory set to what was found, or erased where nothing was.
fn set_found(memories: &mut Memories, kind: MemoryKind, found: Option<EntityId>) {
    match found {
        Some(key) => memories.set(kind, Value::Entity(key)),
        None => memories.erase(kind),
    }
}

/// `PiglinSpecificSensor.findNearestRepellent(level, entity)`: the first block of
/// `#piglin_repellents` (a soul campfire only lit) within 8 sideways and 4 up and down,
/// by Manhattan distance (`findBlocksInBoxByManhattanDistance`).
fn nearest_repellent(cx: &Cx<'_, '_, '_>) -> Option<BlockPos> {
    let world = cx.ai.tick.world;
    crate::mobs::ai::goals::manhattan_ordered(
        cx.ai.block_position(),
        REPELLENT_REACH.0,
        REPELLENT_REACH.1,
        REPELLENT_REACH.0,
    )
    .into_iter()
    .find(|pos| {
        let state = world.state(*pos);
        in_tag(state, "minecraft:piglin_repellents")
            && (!clustine_blocks::data::same_block(state, clustine_data::blocks::SOUL_CAMPFIRE)
                || flag(state, "lit"))
    })
}

/// `PIGLIN_SPECIFIC_SENSOR` (PiglinSpecificSensor.txt): the nearest repellent, and of the
/// living things it sees, nearest first: a baby hoglin, the adult hoglins (counted, the
/// first that may be hunted), the adult piglins and brutes, a player not in gold it may
/// attack, a player holding a loved item, its nemesis (a wither skeleton or the wither),
/// a zombified one; and the adult piglins it lives near.
pub(crate) struct PiglinSpecificSensor;

pub(crate) static PIGLIN_SPECIFIC: PiglinSpecificSensor = PiglinSpecificSensor;

impl Sensor for PiglinSpecificSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[
            MemoryKind::VisibleMobs,
            MemoryKind::Mobs,
            MemoryKind::NearestVisibleNemesis,
            MemoryKind::NearestTargetablePlayerNotWearingGold,
            MemoryKind::NearestPlayerHoldingWantedItem,
            MemoryKind::NearestVisibleHuntableHoglin,
            MemoryKind::NearestVisibleBabyHoglin,
            MemoryKind::NearestVisibleAdultPiglins,
            MemoryKind::NearbyAdultPiglins,
            MemoryKind::VisibleAdultPiglinCount,
            MemoryKind::VisibleAdultHoglinCount,
            MemoryKind::NearestRepellent,
        ]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        match nearest_repellent(cx) {
            Some(pos) => cx
                .memories
                .set(MemoryKind::NearestRepellent, Value::Block(pos)),
            None => cx.memories.erase(MemoryKind::NearestRepellent),
        }
        let mut nemesis = None;
        let mut huntable = None;
        let mut baby_hoglin = None;
        let mut zombified = None;
        let mut not_in_gold = None;
        let mut holding_wanted = None;
        let mut baby_piglin = None;
        let mut hoglins = 0;
        let mut adults = Vec::new();
        for key in visible_mobs(cx.memories).unwrap_or_default() {
            let Some(looked) = find(cx.ai, key) else {
                continue;
            };
            if !targetable(cx, &looked) {
                continue;
            }
            match looked.kind {
                Some(HOGLIN) => {
                    let young = is_baby(cx.ai, &looked);
                    if young && baby_hoglin.is_none() {
                        baby_hoglin = Some(key);
                    } else if !young {
                        hoglins += 1;
                        if huntable.is_none() && hoglin_can_be_hunted(cx, key) {
                            huntable = Some(key);
                        }
                    }
                }
                Some(PIGLIN_BRUTE) => adults.push(key),
                Some(PIGLIN) => {
                    let young = is_baby(cx.ai, &looked);
                    if young && baby_piglin.is_none() {
                        baby_piglin = Some(key);
                    } else if !young {
                        adults.push(key);
                    }
                }
                None => {
                    // A player: their armour is not read (see the module's head).
                    if not_in_gold.is_none()
                        && !piglin::wears_safe_armour(&[])
                        && sensing::can_attack(&looked)
                    {
                        not_in_gold = Some(key);
                    }
                    if holding_wanted.is_none()
                        && !looked.spectator
                        && player_holding_loved(&looked)
                    {
                        holding_wanted = Some(key);
                    }
                }
                Some(kind) if nemesis.is_none() && (kind == WITHER_SKELETON || kind == WITHER) => {
                    nemesis = Some(key);
                }
                Some(_) if zombified.is_none() && is_zombified(&looked) => zombified = Some(key),
                Some(_) => {}
            }
        }
        let _ = baby_piglin;
        let nearby = nearby_adult_piglins(cx);
        set_found(cx.memories, MemoryKind::NearestVisibleNemesis, nemesis);
        set_found(
            cx.memories,
            MemoryKind::NearestVisibleHuntableHoglin,
            huntable,
        );
        set_found(
            cx.memories,
            MemoryKind::NearestVisibleBabyHoglin,
            baby_hoglin,
        );
        set_found(cx.memories, MemoryKind::NearestVisibleZombified, zombified);
        set_found(
            cx.memories,
            MemoryKind::NearestTargetablePlayerNotWearingGold,
            not_in_gold,
        );
        set_found(
            cx.memories,
            MemoryKind::NearestPlayerHoldingWantedItem,
            holding_wanted,
        );
        cx.memories
            .set(MemoryKind::NearbyAdultPiglins, Value::Entities(nearby));
        let count = i32::try_from(adults.len()).unwrap_or(i32::MAX);
        cx.memories.set(
            MemoryKind::NearestVisibleAdultPiglins,
            Value::Entities(adults),
        );
        cx.memories
            .set(MemoryKind::VisibleAdultPiglinCount, Value::Int(count));
        cx.memories
            .set(MemoryKind::VisibleAdultHoglinCount, Value::Int(hoglins));
    }
}

/// `Piglin.BRAIN_PROVIDER` (Piglin.txt's static part): universal anger, eating lately and
/// the spear's memories named; the living, the players, the items it wants, what hurt it
/// and its own sensor, in the game's order.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[
        MemoryKind::UniversalAnger,
        MemoryKind::AteRecently,
        MemoryKind::SpearFleeingTime,
        MemoryKind::SpearFleeingPosition,
        MemoryKind::SpearChargePosition,
        MemoryKind::SpearEngageTime,
    ],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::NEAREST_PLAYERS,
        &NEAREST_ITEMS,
        &sensors::HURT_BY,
        &PIGLIN_SPECIFIC,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The piglins about a player that `angerNearbyPiglins` reaches: those of the type
/// `Piglin` (not its brutes).
pub(crate) fn angered_by_player(kind: u16) -> bool {
    kind == PIGLIN
}
