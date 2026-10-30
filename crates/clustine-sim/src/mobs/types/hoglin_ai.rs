//! The hoglin's brain (`HoglinAi`, `HoglinSpecificSensor`; d HoglinAi.txt and
//! HoglinSpecificSensor.txt) and what the hoglin and the zoglin share of `HoglinBase`
//! (d HoglinBase.txt): the hit that hurts and throws.
//!
//! The brain fights what it is told of or what hurt it, flees piglins that outnumber its
//! kind, keeps away from warped fungi, portals and respawn anchors (`#hoglin_repellents`,
//! which also pacify it for 200 ticks), breeds and follows its adults as a baby. Its
//! activities are made each tick by [`activities`], in `getActivities`' order: core,
//! idle, fight, avoid.
//!
//! A hurt and a word from another hoglin come between the hoglin's ticks, where the game
//! answers them at once (`HoglinAi.wasHurtBy`, `setAttackTargetIfCloserThanCurrent`). Both
//! read where other things are, which only a tick's view knows, so each is kept in the
//! hoglin's own state ([`super::hoglin::Pending`]) and answered at the head of its next
//! tick, before its brain's ([`take_pending`]); the brain's behaviours see them in the
//! same tick as the game's would. **Named**: the memories they write expire one tick
//! later than the game's where the hurt came after the hoglin's own tick, and two hurts
//! in one tick are answered in the order they came.
//!
//! What reads across a line: the word of an attack target goes to every adult hoglin
//! the sensor saw, through `Local::Tell` (`EntityUse::Told` with `ToldWhat::TargetOf`),
//! a copy's to its holder. The word to retreat (`broadcastRetreat`) reads each hoglin's
//! memories from the view and writes them with `Local::SetMemories`, which a hoglin in
//! a copy does not take (named, as the breeding's partner's memories are).

use clustine_blocks::data::in_tag;
use clustine_data::{attributes, entity_types, sounds};
use clustine_world::{Causer, Double, EntityId, Float, HitExtra, HurtBy, Told, ToldWhat};

use crate::mobs::Mob;
use crate::mobs::ai::goals::manhattan_ordered;
use crate::mobs::ai::sensing::{Looked, Sensing};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::{self, Event};
use crate::mobs::brain::behaviours::attacking::{self, MeleeAttack};
use crate::mobs::brain::behaviours::looking::{self, LookAt};
use crate::mobs::brain::behaviours::social::{self, AnimalMakeLove};
use crate::mobs::brain::behaviours::{
    Speed, bottom_centre, cooldowns, entity_of, fleeing, position_of, sample, trigger_if,
    walk_target, walk_to_point, walking,
};
use crate::mobs::brain::sensors::{self, find, visible_mobs};
use crate::mobs::brain::{
    self as machinery, Activity, ActivityData, Behaviour, Brain, BrainState, Cx, DoNothing, Gate,
    HeldShot, HeldTimed, MemoryKind, MemoryWrite, Provider, Sensed, Sensor, Shot, Slot, Status,
    Value,
};
use crate::mobs::part;
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::types::{animal, hoglin};
use crate::physics::{self, Vec3};

/// The hoglin's and the piglin's entity types.
pub(crate) const HOGLIN: u16 = entity_types::HOGLIN as u16;
const PIGLIN: u16 = entity_types::PIGLIN as u16;

/// `REPELLENT_DETECTION_RANGE_HORIZONTAL` and `_VERTICAL`.
const REPELLENT_RANGE: (i32, i32) = (8, 4);
/// `RETREAT_DURATION`: `TimeUtil.rangeOfSeconds(5, 20)`.
const RETREAT_DURATION: (i32, i32) = (100, 400);
/// `ATTACK_DURATION`: how long an attack target is kept.
pub(crate) const ATTACK_DURATION: i64 = 200;
/// `ATTACK_INTERVAL` and `BABY_ATTACK_INTERVAL`.
pub(crate) const ATTACK_INTERVAL: i64 = 40;
pub(crate) const BABY_ATTACK_INTERVAL: i64 = 15;
/// `REPELLENT_PACIFY_TIME`.
const REPELLENT_PACIFY_TIME: i64 = 200;
/// `ADULT_FOLLOW_RANGE`.
const ADULT_FOLLOW_RANGE: (i32, i32) = (5, 16);
/// `DESIRED_DISTANCE_FROM_PIGLIN_WHEN_IDLING` and `_WHEN_RETREATING`.
const DISTANCE_FROM_PIGLIN_IDLING: i32 = 8;
const DISTANCE_FROM_PIGLIN_RETREATING: i32 = 15;
/// What `isOtherTargetMuchFurtherAwayThanCurrentAttackTarget` is asked with.
const MUCH_FURTHER: f64 = 4.0;

/// `HoglinAi.updateActivity`'s list: the first whose conditions hold.
pub(crate) const ACTIVITIES: [Activity; 3] = [Activity::Fight, Activity::Avoid, Activity::Idle];

/// `Hoglin.BRAIN_PROVIDER`: `Brain.provider(sensors, activities)`, its memories those
/// its sensors and behaviours register (Hoglin.txt 636–643).
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::NEAREST_PLAYERS,
        &sensors::NEAREST_ADULT,
        &HOGLIN_SPECIFIC,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

// ------------------------------------------------------------------------------------
// The sensor.

/// `HoglinSpecificSensor` (`HOGLIN_SPECIFIC_SENSOR`): the nearest repellent block, and of
/// the adults it sees the nearest piglin, the hoglins, and how many of each
/// (HoglinSpecificSensor.txt 22–131). A piglin brute is no `Piglin`.
pub(crate) struct HoglinSpecificSensor;

pub(crate) static HOGLIN_SPECIFIC: HoglinSpecificSensor = HoglinSpecificSensor;

impl Sensor for HoglinSpecificSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[
            MemoryKind::VisibleMobs,
            MemoryKind::NearestRepellent,
            MemoryKind::NearestVisibleAdultPiglin,
            MemoryKind::NearestVisibleAdultHoglins,
            MemoryKind::VisibleAdultPiglinCount,
            MemoryKind::VisibleAdultHoglinCount,
        ]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        match nearest_repellent(cx.ai) {
            Some(pos) => cx
                .memories
                .set(MemoryKind::NearestRepellent, Value::Block(pos)),
            None => cx.memories.erase(MemoryKind::NearestRepellent),
        }
        let mut piglin = None;
        let mut piglins = 0;
        let mut hoglins = Vec::new();
        // `NearestVisibleLivingEntities.findAll(predicate)`: nearest first, the predicate
        // (`!isBaby() && (Piglin || Hoglin)`) and then the line of sight.
        for key in visible_mobs(cx.memories).unwrap_or_default() {
            let Some(looked) = find(cx.ai, key) else {
                continue;
            };
            let kind = looked.kind;
            if !matches!(kind, Some(PIGLIN | HOGLIN)) || sensors::is_baby(cx.ai, &looked) {
                continue;
            }
            if !sensors::targetable(cx, &looked) {
                continue;
            }
            if kind == Some(PIGLIN) {
                piglins += 1;
                piglin.get_or_insert(key);
            } else {
                hoglins.push(key);
            }
        }
        match piglin {
            Some(key) => cx
                .memories
                .set(MemoryKind::NearestVisibleAdultPiglin, Value::Entity(key)),
            None => cx.memories.erase(MemoryKind::NearestVisibleAdultPiglin),
        }
        let count = hoglins.len() as i32;
        cx.memories.set(
            MemoryKind::NearestVisibleAdultHoglins,
            Value::Entities(hoglins),
        );
        cx.memories
            .set(MemoryKind::VisibleAdultPiglinCount, Value::Int(piglins));
        cx.memories
            .set(MemoryKind::VisibleAdultHoglinCount, Value::Int(count));
    }
}

/// `findNearestRepellent`: the first block of `#hoglin_repellents` by Manhattan distance
/// within 8 across and 4 up or down of the hoglin's block
/// (`findBlocksInBoxByManhattanDistance(pos, 8, 4)`, n7 LevelReader.txt 387–396).
fn nearest_repellent(ai: &Ai<'_, '_>) -> Option<clustine_world::BlockPos> {
    let (across, up) = REPELLENT_RANGE;
    manhattan_ordered(ai.block_position(), across, up, across)
        .into_iter()
        .find(|&pos| {
            ai.tick
                .world
                .block(pos)
                .is_some_and(|state| in_tag(state, "minecraft:hoglin_repellents"))
        })
}

// ------------------------------------------------------------------------------------
// The behaviours of its own.

/// `BecomePassiveIfMemoryPresent.create(memory, ticks)` (n7
/// BecomePassiveIfMemoryPresent.txt): with no pacification and the memory there,
/// pacified for `ticks` and the attack target forgotten.
struct BecomePassiveIfMemoryPresent {
    memories: [MemoryKind; 3],
    ticks: i64,
}

fn become_passive(memory: MemoryKind, ticks: i64) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(BecomePassiveIfMemoryPresent {
        memories: [MemoryKind::AttackTarget, MemoryKind::Pacified, memory],
        ticks,
    }))
}

impl Shot for BecomePassiveIfMemoryPresent {
    fn required(&self) -> &[MemoryKind] {
        &self.memories
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let [attack, pacified, memory] = self.memories;
        if !cx.memories.check(attack, Status::Registered)
            || !cx.memories.check(pacified, Status::Absent)
            || !cx.memories.check(memory, Status::Present)
        {
            return false;
        }
        cx.memories
            .set_with_expiry(pacified, Value::Bool(true), self.ticks);
        cx.memories.erase(attack);
        true
    }
}

/// `SetWalkTargetAwayFrom.pos(memory, speed, desiredDistance, overrideWalkTarget)` (n7
/// SetWalkTargetAwayFrom.txt): the entity form's rule (`fleeing.rs`) with the block the
/// memory names, at its bottom centre (`Vec3.atBottomCenterOf`).
struct SetWalkTargetAwayFromPos {
    memories: [MemoryKind; 2],
    speed: f32,
    distance: i32,
    override_walk: bool,
}

fn away_from_pos(
    memory: MemoryKind,
    speed: f32,
    distance: i32,
    override_walk: bool,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(SetWalkTargetAwayFromPos {
        memories: [MemoryKind::WalkTarget, memory],
        speed,
        distance,
        override_walk,
    }))
}

impl Shot for SetWalkTargetAwayFromPos {
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
        let Some(Value::Block(block)) = cx.memories.get(memory) else {
            return false;
        };
        let target = bottom_centre(block);
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
            if let Some(at) = crate::mobs::ai::random_pos::land_pos_away(cx.ai, 16, 7, target) {
                cx.memories
                    .set(MemoryKind::WalkTarget, walk_to_point(at, self.speed, 0));
                break;
            }
        }
        true
    }
}

/// `MeleeAttack.create(cooldown)` whose hit is the hoglin's `doHurtTarget`.
fn melee(cooldown: i64) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(MeleeAttack {
        predicate: |_| true,
        cooldown,
        hit: hoglin_hits,
    }))
}

/// `createIdleMovementBehaviors()`: a stroll, a walk to the look target, or nothing, by
/// weights 2, 2 and 1 (HoglinAi.txt 159–184; the zoglin's idle is the same).
pub(crate) fn idle_movement() -> Box<dyn Behaviour> {
    Box::new(Gate::run_one(vec![
        (walking::stroll(0.4), 2),
        (walking::walk_to_look_target(0.4, 3), 2),
        (Box::new(HeldTimed::new(DoNothing { min: 30, max: 60 })), 1),
    ]))
}

/// `HoglinAi.getActivities()` (HoglinAi.txt 39–157).
fn activities() -> Vec<ActivityData> {
    use MemoryKind::{AttackTarget, AvoidTarget, NearestRepellent, NearestVisibleAdultPiglin};
    let core = ActivityData::numbered(
        Activity::Core,
        0,
        vec![walking::look_sink(45, 90), walking::sink()],
    );
    let idle = ActivityData::numbered(
        Activity::Idle,
        10,
        vec![
            become_passive(NearestRepellent, REPELLENT_PACIFY_TIME),
            AnimalMakeLove::at(HOGLIN, 0.6, 2).boxed(),
            away_from_pos(NearestRepellent, 1.0, 8, true),
            attacking::start_attacking(find_nearest_valid_attack_target),
            trigger_if(
                is_adult,
                fleeing::away_from(
                    NearestVisibleAdultPiglin,
                    0.4,
                    DISTANCE_FROM_PIGLIN_IDLING,
                    false,
                ),
            ),
            looking::look_sometimes(LookAt::Any, 8.0, (30, 60)),
            social::follow_adult(ADULT_FOLLOW_RANGE, 0.6),
            idle_movement(),
        ],
    );
    let fight = ActivityData::while_present(
        Activity::Fight,
        10,
        vec![
            become_passive(NearestRepellent, REPELLENT_PACIFY_TIME),
            AnimalMakeLove::at(HOGLIN, 0.6, 2).boxed(),
            attacking::walk_to_attack_target(Speed::Fixed(1.0)),
            trigger_if(is_adult, melee(ATTACK_INTERVAL)),
            trigger_if(is_baby, melee(BABY_ATTACK_INTERVAL)),
            attacking::stop_attacking(),
            cooldowns::erase_if(is_breeding, AttackTarget),
        ],
        AttackTarget,
    );
    let avoid = ActivityData::while_present(
        Activity::Avoid,
        10,
        vec![
            fleeing::away_from(AvoidTarget, 1.3, DISTANCE_FROM_PIGLIN_RETREATING, false),
            idle_movement(),
            looking::look_sometimes(LookAt::Any, 8.0, (30, 60)),
            cooldowns::erase_if(wants_to_stop_fleeing, AvoidTarget),
        ],
        AvoidTarget,
    );
    vec![core, idle, fight, avoid]
}

// ------------------------------------------------------------------------------------
// The brain's predicates and finders.

/// `isBaby()` of the hoglin whose brain ticks.
fn is_baby(cx: &mut Cx<'_, '_, '_>) -> bool {
    animal::is_baby(&cx.ai.tick.mob)
}

/// `Hoglin.isAdult()`.
fn is_adult(cx: &mut Cx<'_, '_, '_>) -> bool {
    !is_baby(cx)
}

/// `HoglinAi.isBreeding`: a breed target remembered.
fn is_breeding(cx: &mut Cx<'_, '_, '_>) -> bool {
    cx.memories.check(MemoryKind::BreedTarget, Status::Present)
}

/// `HoglinAi.isPacified`.
fn is_pacified(cx: &Cx<'_, '_, '_>) -> bool {
    cx.memories.check(MemoryKind::Pacified, Status::Present)
}

/// `HoglinAi.findNearestValidAttackTarget`: none while pacified or breeding, else the
/// nearest attackable player it sees (none in creative).
fn find_nearest_valid_attack_target(cx: &mut Cx<'_, '_, '_>) -> Option<EntityId> {
    if is_pacified(cx) || is_breeding(cx) {
        return None;
    }
    entity_of(cx, MemoryKind::NearestVisibleTargetablePlayer)
}

/// `HoglinAi.piglinsOutnumberHoglins`: never for a baby; else the adult piglins it sees
/// against the adult hoglins it sees and itself.
fn piglins_outnumber_hoglins(cx: &Cx<'_, '_, '_>) -> bool {
    if animal::is_baby(&cx.ai.tick.mob) {
        return false;
    }
    let piglins = cx
        .memories
        .int(MemoryKind::VisibleAdultPiglinCount)
        .unwrap_or(0);
    let hoglins = cx
        .memories
        .int(MemoryKind::VisibleAdultHoglinCount)
        .unwrap_or(0)
        + 1;
    piglins > hoglins
}

/// `HoglinAi.wantsToStopFleeing`: an adult no longer outnumbered.
fn wants_to_stop_fleeing(cx: &mut Cx<'_, '_, '_>) -> bool {
    !animal::is_baby(&cx.ai.tick.mob) && !piglins_outnumber_hoglins(cx)
}

/// `NEAREST_VISIBLE_ADULT_HOGLINS` as the memories hold it, none where it is not there
/// (`getVisibleAdultHoglins`).
fn visible_adult_hoglins(memories: &machinery::Memories) -> Vec<EntityId> {
    match memories.get(MemoryKind::NearestVisibleAdultHoglins) {
        Some(Value::Entities(list)) => list,
        _ => Vec::new(),
    }
}

/// `NEAREST_VISIBLE_ADULT_HOGLINS` as the region keeps it beside the record, for what
/// is done outside the brain's tick.
fn kept_adult_hoglins(sensed: Option<&Sensed>) -> Vec<EntityId> {
    match sensed.and_then(|sensed| sensed.get(&MemoryKind::NearestVisibleAdultHoglins)) {
        Some(Slot::Kept {
            value: Value::Entities(list),
            ..
        }) => list.clone(),
        _ => Vec::new(),
    }
}

// ------------------------------------------------------------------------------------
// Updating the activity, and its sounds.

/// `HoglinAi.updateActivity` after the brain's tick (HoglinAi.txt 186–225): the first
/// of fight, avoid and idle whose conditions hold; where that changed it, the sound of
/// the new one; and its aggression as an attack target is remembered.
pub(crate) fn update_activity(brain: &mut Brain, ai: &mut Ai<'_, '_>) {
    let old = brain.active_non_core();
    brain.set_active_to_first_valid(&ACTIVITIES);
    let new = brain.active_non_core();
    if old != new
        && let Some(activity) = new
    {
        let near_repellent = brain
            .memories
            .check(MemoryKind::NearestRepellent, Status::Present);
        let sound = sound_for_activity(activity, hoglin::is_converting(ai.tick), near_repellent);
        body::make_sound(ai.tick, Some(sound), 1.0);
    }
    ai.tick.mob.body.combat.aggressive = brain
        .memories
        .check(MemoryKind::AttackTarget, Status::Present);
}

/// `HoglinAi.getSoundForActivity` (HoglinAi.txt 517–538): its retreat while avoiding or
/// converting, its anger in a fight, its retreat near a repellent, else its ambient.
pub(crate) fn sound_for_activity(
    activity: Activity,
    converting: bool,
    near_repellent: bool,
) -> i32 {
    if activity == Activity::Avoid || converting {
        sounds::ENTITY_HOGLIN_RETREAT
    } else if activity == Activity::Fight {
        sounds::ENTITY_HOGLIN_ANGRY
    } else if near_repellent {
        sounds::ENTITY_HOGLIN_RETREAT
    } else {
        sounds::ENTITY_HOGLIN_AMBIENT
    }
}

/// The active activity beside the core one as the record has it: idle before the first
/// tick, as the brain's construction makes it.
pub(crate) fn recorded_activity(mob: &Mob) -> Activity {
    part::decode::<BrainState>(&mob.mind.brain)
        .ok()
        .and_then(|state| state.active)
        .unwrap_or(Activity::Idle)
}

// ------------------------------------------------------------------------------------
// The hit, shared with the zoglin.

/// `Hoglin.doHurtTarget` (Hoglin.txt 84–113): its arms shown (entity event 4), its
/// attack's sound, `HoglinAi.onHitTarget`, and the hurt that throws.
fn hoglin_hits(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    if !sensors::is_living(target) {
        return;
    }
    // `attackAnimationRemainingTicks = 10`: read only by clients, which entity event 4
    // sets it for.
    cx.ai.tick.out.events.push(Event::Entity(4));
    body::make_sound(cx.ai.tick, Some(sounds::ENTITY_HOGLIN_ATTACK), 1.0);
    on_hit_target(cx, target);
    let baby = animal::is_baby(&cx.ai.tick.mob);
    hurt_and_throw_target(cx.ai, target, baby);
}

/// `HoglinBase.hurtAndThrowTarget(level, attacker, target)` (HoglinBase.txt 9–60): an
/// adult's hurt is half its attack damage and a draw of the whole of it, a baby's its
/// attack damage; `mobAttack` of the attacker, and where it took, an adult's throw
/// (`throwTarget`, by its attack knockback, `HitExtra::Throw`), which the lander draws
/// from the hit's stream. A creative player refuses it, and so is never thrown.
pub(crate) fn hurt_and_throw_target(ai: &mut Ai<'_, '_>, target: &Looked, baby: bool) {
    let damage = ai.attribute(attributes::ATTACK_DAMAGE) as f32;
    let whole = damage as i32;
    let amount = if !baby && whole > 0 {
        damage / 2.0 + ai.tick.draws.next_int(whole as u32) as f32
    } else {
        damage
    };
    let knockback = ai.attribute(attributes::ATTACK_KNOCKBACK) as f32;
    let extras = if baby {
        Vec::new()
    } else {
        vec![HitExtra::Throw(Float(knockback))]
    };
    let attacker = Causer {
        key: ai.tick.key,
        kind: ai.tick.kind,
        position: ai.position().map(Double),
        player: None,
        living: true,
    };
    let hurt = HurtBy {
        damage_type: "minecraft:mob_attack".to_owned(),
        cause: Some(attacker.clone()),
        direct: Some(attacker),
        position: None,
        projectile_motion: None,
        amount: Float(amount),
        knockback: None,
        fire_before: 0,
        fire_after: 0,
        effects: Vec::new(),
        arrow: false,
        extras,
        sound: None,
        hit: Some((ai.tick.key, ai.tick.lived)),
    };
    ai.tick.out.local.push(Local::Strike {
        key: target.key,
        hurt: Box::new(hurt),
    });
}

/// `HoglinAi.onHitTarget` (HoglinAi.txt 227–250): an adult that hits a piglin while
/// piglins outnumber its kind retreats from it and has the hoglins it sees retreat too;
/// any other hit is the target the hoglins it sees are told of.
fn on_hit_target(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    if animal::is_baby(&cx.ai.tick.mob) {
        return;
    }
    let hoglins = visible_adult_hoglins(cx.memories);
    if target.kind == Some(PIGLIN) && piglins_outnumber_hoglins(cx) {
        set_avoid_target(cx, target.key);
        broadcast_retreat(cx, &hoglins, target);
        return;
    }
    broadcast_attack_target(cx.ai, &hoglins, target);
}

// ------------------------------------------------------------------------------------
// Targets: set, compared and told.

/// `HoglinAi.setAvoidTarget`: no attack target or walk, and `key` avoided for a time
/// drawn of the level's random (`RETREAT_DURATION`).
fn set_avoid_target(cx: &mut Cx<'_, '_, '_>, key: EntityId) {
    cx.memories.erase(MemoryKind::AttackTarget);
    cx.memories.erase(MemoryKind::WalkTarget);
    let ttl = sample(cx.ai.tick.draws, RETREAT_DURATION);
    cx.memories
        .set_with_expiry(MemoryKind::AvoidTarget, Value::Entity(key), i64::from(ttl));
}

/// `HoglinAi.setAttackTarget`: no unreachable walk or breed target, and `key` attacked
/// for `ATTACK_DURATION`.
fn set_attack_target(cx: &mut Cx<'_, '_, '_>, key: EntityId) {
    cx.memories.erase(MemoryKind::CantReachWalkTargetSince);
    cx.memories.erase(MemoryKind::BreedTarget);
    cx.memories.set_with_expiry(
        MemoryKind::AttackTarget,
        Value::Entity(key),
        ATTACK_DURATION,
    );
}

/// `BehaviorUtils.getNearestTarget(mob, current, candidate)` from `from`: the current
/// one where it is strictly nearer, else the candidate (n7 BehaviorUtils.txt 330–364).
/// A current one the view has not is passed over.
fn nearest_of(ai: &Ai<'_, '_>, from: Vec3, current: Option<EntityId>, candidate: Looked) -> Looked {
    match current.and_then(|key| find(ai, key)) {
        Some(current)
            if physics::distance_sqr(from, current.position)
                < physics::distance_sqr(from, candidate.position) =>
        {
            current
        }
        _ => candidate,
    }
}

/// `BehaviorUtils.isOtherTargetMuchFurtherAwayThanCurrentAttackTarget(mob, other, 4)`:
/// whether `other` is farther than the remembered attack target by more than 4 squared
/// (n7 BehaviorUtils.txt 273–308); false with none, or one the view has not.
pub(crate) fn much_further_than_target(cx: &Cx<'_, '_, '_>, other: &Looked) -> bool {
    let Some(current) = entity_of(cx, MemoryKind::AttackTarget).and_then(|key| find(cx.ai, key))
    else {
        return false;
    };
    let from = cx.ai.position();
    physics::distance_sqr(from, other.position)
        > physics::distance_sqr(from, current.position) + MUCH_FURTHER * MUCH_FURTHER
}

/// What `target` is to those told of it: its key, type, a player's UUID and its eyes.
fn told_of(ai: &Ai<'_, '_>, target: &Looked) -> Told {
    let player = if target.is_player() {
        ai.tick
            .world
            .players()
            .iter()
            .find(|player| player.key == target.key)
            .map(|player| uuid::Uuid::from_u128(player.uuid))
    } else {
        None
    };
    Told {
        what: ToldWhat::TargetOf,
        of: target.key,
        kind: target.kind.unwrap_or(entity_types::PLAYER as u16),
        player,
        eyes: [target.position[0], target.eye_y, target.position[2]].map(Double),
    }
}

/// `HoglinAi.broadcastAttackTarget`: each of `hoglins` told to take `target` where it is
/// nearer than its own (`setAttackTargetIfCloserThanCurrent`, which the told hoglin
/// answers at its next tick).
fn broadcast_attack_target(ai: &mut Ai<'_, '_>, hoglins: &[EntityId], target: &Looked) {
    let told = told_of(ai, target);
    for &key in hoglins {
        ai.tick.out.local.push(Local::Tell {
            key,
            told: Box::new(told.clone()),
        });
    }
}

/// `HoglinAi.broadcastRetreat`: each of `hoglins` retreats from the nearest of its avoid
/// target, its attack target and `piglin` (`retreatFromNearestTarget`), its memories
/// read from the view and written after the tick; one the view has not is passed over.
fn broadcast_retreat(cx: &mut Cx<'_, '_, '_>, hoglins: &[EntityId], piglin: &Looked) {
    for &key in hoglins {
        let Some(seen) = cx.ai.tick.world.entity(key) else {
            continue;
        };
        let (from, lived) = (seen.entity.position, seen.entity.lived);
        let Ok(other) = Mob::decode(&seen.entity.data) else {
            continue;
        };
        let memories = machinery::memories_of(&PROVIDER, &other, lived);
        let of = |kind| match memories.get(kind) {
            Some(Value::Entity(key)) => Some(key),
            _ => None,
        };
        let nearest = nearest_of(cx.ai, from, of(MemoryKind::AvoidTarget), piglin.clone());
        let nearest = nearest_of(cx.ai, from, of(MemoryKind::AttackTarget), nearest);
        let ttl = sample(cx.ai.tick.draws, RETREAT_DURATION);
        cx.ai.tick.out.local.push(Local::SetMemories {
            key,
            writes: vec![
                MemoryWrite::Erase(MemoryKind::AttackTarget),
                MemoryWrite::Erase(MemoryKind::WalkTarget),
                MemoryWrite::SetFor(
                    MemoryKind::AvoidTarget,
                    Value::Entity(nearest.key),
                    i64::from(ttl),
                ),
            ],
        });
    }
}

// ------------------------------------------------------------------------------------
// What comes between its ticks.

/// The hurts and words a hoglin was given since its last tick, answered at the head of
/// this one in the order they came, before its brain's tick: `HoglinAi.wasHurtBy` of a
/// living attacker (Hoglin.txt 125–149) and `setAttackTargetIfCloserThanCurrent` of a
/// target another hoglin told it of.
pub(crate) fn take_pending(tick: &mut MobTick<'_>) {
    let pending = hoglin::take_pending(&mut tick.mob);
    if pending.is_empty() {
        return;
    }
    let lived = tick.lived;
    let time = tick.world.game_time();
    let avoiding = recorded_activity(&tick.mob) == Activity::Avoid;
    let hoglins = kept_adult_hoglins(tick.sensed.as_ref());
    let mut memories = machinery::memories_of(&PROVIDER, &tick.mob, lived);
    let mut setup = Setup::default();
    tick.mob_type.register_goals(&mut setup);
    let config = setup.config;
    {
        let mut ai = Ai {
            tick: &mut *tick,
            config: &config,
            sensing: Sensing::default(),
            navigation_ticked: true,
        };
        let mut cx = Cx {
            ai: &mut ai,
            memories: &mut memories,
            time,
        };
        for each in pending {
            match each {
                hoglin::Pending::HurtBy { key, kind } => {
                    was_hurt_by(&mut cx, key, kind, avoiding, &hoglins);
                }
                hoglin::Pending::TargetOf(key) => set_attack_target_if_closer(&mut cx, key),
            }
        }
    }
    machinery::write_memories(&PROVIDER, &mut tick.mob, memories);
}

/// `HoglinAi.wasHurtBy(level, hoglin, attacker)`: no pacification or breed target; a
/// baby retreats from the nearest of what it avoids, attacks and the attacker; an adult
/// retaliates (`maybeRetaliate`): not against a piglin while it avoids, nor a hoglin,
/// nor one much farther than its target, nor one it may not attack; else the attacker
/// is its target and the hoglins it sees are told of it.
fn was_hurt_by(
    cx: &mut Cx<'_, '_, '_>,
    key: EntityId,
    kind: u16,
    avoiding: bool,
    hoglins: &[EntityId],
) {
    cx.memories.erase(MemoryKind::Pacified);
    cx.memories.erase(MemoryKind::BreedTarget);
    let Some(attacker) = find(cx.ai, key).filter(sensors::is_living) else {
        return;
    };
    if animal::is_baby(&cx.ai.tick.mob) {
        let from = cx.ai.position();
        let nearest = nearest_of(
            cx.ai,
            from,
            entity_of(cx, MemoryKind::AvoidTarget),
            attacker,
        );
        let nearest = nearest_of(
            cx.ai,
            from,
            entity_of(cx, MemoryKind::AttackTarget),
            nearest,
        );
        set_avoid_target(cx, nearest.key);
        return;
    }
    if (avoiding && kind == PIGLIN) || kind == HOGLIN {
        return;
    }
    if much_further_than_target(cx, &attacker) || !sensors::attackable(cx, &attacker) {
        return;
    }
    set_attack_target(cx, attacker.key);
    broadcast_attack_target(cx.ai, hoglins, &attacker);
}

/// `HoglinAi.setAttackTargetIfCloserThanCurrent(hoglin, target)`: not while pacified;
/// else the nearer of its attack target and `key` is its target for
/// `ATTACK_DURATION` again. A target the view has not is not taken.
fn set_attack_target_if_closer(cx: &mut Cx<'_, '_, '_>, key: EntityId) {
    if is_pacified(cx) {
        return;
    }
    let Some(candidate) = find(cx.ai, key) else {
        return;
    };
    let from = cx.ai.position();
    let nearest = nearest_of(
        cx.ai,
        from,
        entity_of(cx, MemoryKind::AttackTarget),
        candidate,
    );
    set_attack_target(cx, nearest.key);
}

/// Whether `pos` is within 8 of the repellent the region keeps beside the hoglin's
/// record (`HoglinAi.isPosNearNearestRepellent`: `BlockPos.closerThan(pos, 8)`).
pub(crate) fn near_kept_repellent(sensed: Option<&Sensed>, pos: clustine_world::BlockPos) -> bool {
    let Some(Slot::Kept {
        value: Value::Block(repellent),
        ..
    }) = sensed.and_then(|sensed| sensed.get(&MemoryKind::NearestRepellent))
    else {
        return false;
    };
    let (dx, dy, dz) = (
        f64::from(repellent.x - pos.x),
        f64::from(repellent.y - pos.y),
        f64::from(repellent.z - pos.z),
    );
    dx * dx + dy * dy + dz * dz < 64.0
}

/// Whether the region keeps a repellent beside the hoglin's record (`isNearRepellent`).
pub(crate) fn kept_repellent(sensed: Option<&Sensed>) -> bool {
    sensed.is_some_and(|sensed| sensed.contains_key(&MemoryKind::NearestRepellent))
}
