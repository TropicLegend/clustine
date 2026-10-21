//! The axolotl's brain (`AxolotlAi`, `PlayDead`, `ValidatePlayDead` and
//! `AxolotlAttackablesSensor`; `docs/groundwork/n7-brief.md`, sections 7.4.6 to 7.4.8):
//! in its core it looks, walks, plays dead for as long as a hurt said and lets its
//! temptation cool; idle it looks at players, breeds, follows a player's bucket of
//! tropical fish or an adult, hunts, goes back to water and strolls; it fights what it
//! hunts and, hit in water, plays dead.
//!
//! What reads across a line (`docs/groundwork/n7j-plan.md`, section 3): its sensors see
//! the region's land, copies and faces (item 1); an attack target, a look target, a walk
//! target or an adult only a copy shows is followed and hit as the shared behaviours do
//! (item 2, a hit across as `EntityUse::MobHit`, item 5); breeding with a partner only a
//! copy shows is the region's protocol (item 4). "Unknown" is never dead (the plan's
//! section 8 item 8): a target its region's view has not is kept by the shared readers,
//! and the hunt's end of an attack target the view has not gives nothing, as it gives
//! nothing in any case here.
//!
//! Named differences:
//!
//! - `ValidatePlayDead` ends playing dead with `useDefaultActivity`, which no behaviour
//!   of the machinery can call: the axolotl's activity update does it in the same tick,
//!   after the passes, so that the idle behaviours of the priorities after
//!   `ValidatePlayDead`'s start a tick later than the game's.
//! - No mob effects (the plan's 1.2): playing dead gives no Regeneration, and a hunt's end
//!   gives the player none and takes no Mining Fatigue away (`onStopAttacking` does
//!   nothing).
//! - Whether a thing it would hunt is in water is its box's fluids as the region has them
//!   now, where the game reads what the thing's own last tick found.

use clustine_blocks::data::{entity_in_tag, fluid_in_tag, item_in_tag};
use clustine_data::{entity_types, sounds};
use clustine_world::EntityId;

use crate::mobs::Mob;
use crate::mobs::ai::sensing::Looked;
use crate::mobs::brain::behaviours::attacking::{
    MeleeAttack, StartAttacking, StopAttackingIfTargetInvalid, walk_to_attack_target,
};
use crate::mobs::brain::behaviours::cooldowns::{count_down, erase_if};
use crate::mobs::brain::behaviours::liquids::find_liquid;
use crate::mobs::brain::behaviours::looking::{LookAt, look_sometimes};
use crate::mobs::brain::behaviours::social::{AnimalMakeLove, FollowTemptation, follow_adult_by};
use crate::mobs::brain::behaviours::walking::{
    SetWalkTargetFromLookTarget, StrollTo, look_sink, sink, stroll_to,
};
use crate::mobs::brain::behaviours::{Speed, block_of, entity_of, look_target};
use crate::mobs::brain::sensors::{
    self, HURT_BY, NEAREST_ADULT, NEAREST_LIVING_ENTITIES, TemptingSensor, find_closest_visible,
};
use crate::mobs::brain::{
    Activity, ActivityData, Behaviour, Brain, Cx, Gate, HeldShot, HeldTimed, MemoryKind, Order,
    Provider, Running, Sensor, Shot, Slot, Status, Timed, Value,
};
use crate::mobs::tick::{Local, MobTick};
use crate::physics;

/// `BRAIN_PROVIDER`: its five sensors, and `AxolotlAi.getActivities`.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[],
    sensors: &[
        &NEAREST_LIVING_ENTITIES,
        &NEAREST_ADULT,
        &HURT_BY,
        &AXOLOTL_ATTACKABLES,
        &FOOD_TEMPTATIONS,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `HAS_HUNTING_COOLDOWN`'s time after a fight ends.
pub(crate) const HUNTING_COOLDOWN: i64 = 2400;

/// `TemptingSensor.forAnimal(axolotl)`: `isFood`, `#axolotl_food`.
pub(crate) static FOOD_TEMPTATIONS: TemptingSensor = TemptingSensor {
    tempts: |_, stack| item_in_tag(stack.item, "minecraft:axolotl_food"),
};

/// `AxolotlAttackablesSensor` (`AXOLOTL_ATTACKABLES`): of the seen living things, the
/// closest within 8 blocks and in water that is always its enemy (`#axolotl_always_
/// hostiles`), or that it hunts (`#axolotl_hunt_targets`) while it has no hunting
/// cooldown, and that it may attack, as `nearest_attackable`.
pub(crate) struct AxolotlAttackablesSensor;

pub(crate) static AXOLOTL_ATTACKABLES: AxolotlAttackablesSensor = AxolotlAttackablesSensor;

/// Whether the thing `looked` is in water (`isInWater`): its box's fluids as the region
/// has them.
pub(crate) fn looked_in_water(cx: &Cx<'_, '_, '_>, looked: &Looked) -> bool {
    physics::fluids::update(cx.ai.tick.world, &looked.bounds).in_water()
}

/// Whether the thing `looked` is of the entity tag `tag` (a player is of none of these).
fn kind_in_tag(looked: &Looked, tag: &str) -> bool {
    looked
        .kind
        .is_some_and(|kind| entity_in_tag(i32::from(kind), tag))
}

impl AxolotlAttackablesSensor {
    /// `isMatchingEntity(level, axolotl, entity)`, its tests in the game's order.
    fn matching(cx: &mut Cx<'_, '_, '_>, looked: &Looked, cooling: bool) -> bool {
        let close = physics::distance_sqr(looked.position, cx.ai.position()) <= 64.0;
        close
            && looked_in_water(cx, looked)
            && (kind_in_tag(looked, "minecraft:axolotl_always_hostiles")
                || (!cooling && kind_in_tag(looked, "minecraft:axolotl_hunt_targets")))
            && sensors::attackable(cx, looked)
    }
}

impl Sensor for AxolotlAttackablesSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[
            MemoryKind::NearestAttackable,
            MemoryKind::VisibleMobs,
            MemoryKind::HasHuntingCooldown,
        ]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        // `setMemory(type, Optional)`: none where nothing is seen.
        if sensors::visible_mobs(cx.memories).is_none() {
            cx.memories.erase(MemoryKind::NearestAttackable);
            return;
        }
        let cooling = cx
            .memories
            .check(MemoryKind::HasHuntingCooldown, Status::Present);
        let found = find_closest_visible(cx, &mut |cx, looked| Self::matching(cx, looked, cooling));
        match found {
            Some(found) => cx
                .memories
                .set(MemoryKind::NearestAttackable, Value::Entity(found.key)),
            None => cx.memories.erase(MemoryKind::NearestAttackable),
        }
    }
}

/// `BehaviorBuilder.triggerIf(predicate)`: a one-shot that names no memory and fires
/// where the predicate holds, doing nothing: the "stand still" of a gate.
pub(crate) struct StandStill {
    pub(crate) predicate: fn(&Cx<'_, '_, '_>) -> bool,
}

impl Shot for StandStill {
    fn required(&self) -> &[MemoryKind] {
        &[]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        (self.predicate)(cx)
    }
}

/// [`StandStill`], boxed.
pub(crate) fn stand_still(predicate: fn(&Cx<'_, '_, '_>) -> bool) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(StandStill { predicate }))
}

/// `Entity::isInWater`.
pub(crate) fn in_water(cx: &Cx<'_, '_, '_>) -> bool {
    cx.ai.in_water()
}

/// `Entity::onGround`.
pub(crate) fn on_ground(cx: &Cx<'_, '_, '_>) -> bool {
    cx.ai.on_ground()
}

/// `BehaviorUtils.isBreeding(mob)`: a breed target.
pub(crate) fn breeding(cx: &mut Cx<'_, '_, '_>) -> bool {
    cx.memories.check(MemoryKind::BreedTarget, Status::Present)
}

/// `getSpeedModifierChasing` and `getSpeedModifierFollowingAdult`: 0.6 in water, 0.15
/// out.
fn chasing_speed(cx: &Cx<'_, '_, '_>) -> f32 {
    if cx.ai.in_water() { 0.6 } else { 0.15 }
}

/// `getSpeedModifier`: 0.5 in water, 0.15 out.
fn idle_speed(cx: &Cx<'_, '_, '_>) -> f32 {
    if cx.ai.in_water() { 0.5 } else { 0.15 }
}

/// `findNearestValidAttackTarget`: none while breeding, else `nearest_attackable`.
fn nearest_valid_attack_target(cx: &mut Cx<'_, '_, '_>) -> Option<EntityId> {
    if breeding(cx) {
        return None;
    }
    entity_of(cx, MemoryKind::NearestAttackable)
}

/// `canSetWalkTargetFromLookTarget`: there is a look target, and whether water is at its
/// block is whether the axolotl is in water. A look target the view has not is no
/// block, and so no walk this tick.
fn can_walk_to_look_target(cx: &Cx<'_, '_, '_>) -> bool {
    let Some(pos) = look_target(cx).and_then(|tracker| block_of(cx, &tracker)) else {
        return false;
    };
    let water = fluid_in_tag(cx.ai.tick.world.state(pos), "minecraft:water");
    water == cx.ai.in_water()
}

fn never(_: &mut Cx<'_, '_, '_>, _: &Looked) -> bool {
    false
}

/// `Axolotl.onStopAttacking(level, axolotl, target)`: a player who finished the target
/// off near it is given Regeneration and freed of Mining Fatigue, which is all it does;
/// without mob effects (the plan's 1.2) nothing is done.
fn on_stop_attacking(_: &mut Cx<'_, '_, '_>, _: EntityId) {}

/// `Mob.doHurtTarget` of an axolotl: the hit of its attack damage, and where it took,
/// `playAttackSound` (`AXOLOTL_ATTACK` at 1 and 1).
fn hit(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    let amount = cx.ai.attribute(clustine_data::attributes::ATTACK_DAMAGE) as f32;
    cx.ai.tick.out.local.push(Local::Hurt {
        key: target.key,
        kind: cx.ai.tick.kind,
        from: cx.ai.position(),
        amount,
        sound: Some(sounds::ENTITY_AXOLOTL_ATTACK),
        knockback: None,
    });
}

fn any(_: &Cx<'_, '_, '_>) -> bool {
    true
}

/// `ValidatePlayDead.create()`: while `play_dead_ticks` is there, one less a tick; at
/// nothing, it and `hurt_by_entity` are forgotten and the default activity is taken
/// (which [`update_activity`] does, see the module's comment). The count is a countdown
/// the machinery lowers ([`crate::mobs::brain::Countdown`]): its lowering begins at its
/// first tick and is told each tick after, so that nothing is written while it plays
/// dead.
pub(crate) struct ValidatePlayDead;

impl Shot for ValidatePlayDead {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::PlayDeadTicks, MemoryKind::HurtByEntity]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::PlayDeadTicks, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::HurtByEntity, Status::Registered)
        {
            return false;
        }
        let ticks = cx.memories.int(MemoryKind::PlayDeadTicks).unwrap_or(0);
        if ticks <= 0 {
            cx.memories.erase(MemoryKind::PlayDeadTicks);
            cx.memories.erase(MemoryKind::HurtByEntity);
        } else {
            cx.memories.begin_lowering(MemoryKind::PlayDeadTicks);
            cx.memories.lower(MemoryKind::PlayDeadTicks);
        }
        true
    }
}

/// `PlayDead`: hurt by something in water, it lies still for up to 200 ticks while it
/// stays in water and plays dead, its walk and look forgotten as it starts.
pub(crate) struct PlayDead;

impl Timed for PlayDead {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::PlayDeadTicks, Status::Present),
            (MemoryKind::HurtByEntity, Status::Present),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (200, 200)
    }

    fn extra_start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.ai.in_water()
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.ai.in_water()
            && cx
                .memories
                .check(MemoryKind::PlayDeadTicks, Status::Present)
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::WalkTarget);
        cx.memories.erase(MemoryKind::LookTarget);
        // `addEffect(REGENERATION, 200)`: no mob effects yet (named).
    }
}

/// The activities, in `getActivities`' order: core, idle, fight and play dead.
fn activities() -> Vec<ActivityData> {
    vec![core(), idle(), fight(), play_dead()]
}

/// `initCoreActivity`.
fn core() -> ActivityData {
    ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            look_sink(45, 90),
            sink(),
            Box::new(HeldShot::new(ValidatePlayDead)),
            count_down(MemoryKind::TemptationCooldownTicks),
        ],
    )
}

/// `initIdleActivity`, with its own priorities.
fn idle() -> ActivityData {
    let follow = FollowTemptation::new(Speed::Of(idle_speed)).boxed();
    let adult = follow_adult_by(
        (5, 16),
        Speed::Of(chasing_speed),
        MemoryKind::NearestVisibleAdult,
        false,
    );
    let wander = Gate::new(
        vec![(MemoryKind::WalkTarget, Status::Absent)],
        Vec::new(),
        Order::Ordered,
        Running::TryAll,
        vec![
            (stroll_to(0.5, StrollTo::Swim), 2),
            (
                stroll_to(
                    0.15,
                    StrollTo::Land {
                        horizontal: 10,
                        vertical: 7,
                        in_water: false,
                    },
                ),
                2,
            ),
            (
                Box::new(HeldShot::new(SetWalkTargetFromLookTarget {
                    predicate: can_walk_to_look_target,
                    speed: Speed::Of(idle_speed),
                    close_enough: 3,
                })),
                3,
            ),
            (stand_still(in_water), 5),
            (stand_still(on_ground), 5),
        ],
    );
    ActivityData {
        activity: Activity::Idle,
        behaviours: vec![
            (0, look_sometimes(LookAt::Player, 6.0, (30, 60))),
            (
                1,
                AnimalMakeLove::at(entity_types::AXOLOTL as u16, 0.2, 2).boxed(),
            ),
            (2, Box::new(Gate::run_one(vec![(follow, 1), (adult, 1)]))),
            (
                3,
                Box::new(HeldShot::new(StartAttacking {
                    condition: |_| true,
                    finder: nearest_valid_attack_target,
                })),
            ),
            (3, find_liquid(6, 0.15, "minecraft:axolotl_tries_to_find")),
            (4, Box::new(wander)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// `initFightActivity`: while it has an attack target, which it forgets as it stops.
fn fight() -> ActivityData {
    ActivityData::while_present(
        Activity::Fight,
        0,
        vec![
            Box::new(HeldShot::new(StopAttackingIfTargetInvalid {
                condition: never,
                callback: on_stop_attacking,
                tired: true,
            })),
            walk_to_attack_target(Speed::Of(chasing_speed)),
            Box::new(HeldShot::new(MeleeAttack {
                predicate: any,
                cooldown: 20,
                hit,
            })),
            erase_if(breeding, MemoryKind::AttackTarget),
        ],
        MemoryKind::AttackTarget,
    )
}

/// `initPlayDeadActivity`: while `play_dead_ticks` is there, which it forgets as it
/// stops.
fn play_dead() -> ActivityData {
    ActivityData {
        activity: Activity::PlayDead,
        behaviours: vec![
            (0, Box::new(HeldTimed::new(PlayDead))),
            (1, erase_if(breeding, MemoryKind::PlayDeadTicks)),
        ],
        conditions: vec![(MemoryKind::PlayDeadTicks, Status::Present)],
        erase: vec![MemoryKind::PlayDeadTicks],
    }
}

/// The memory `kind`'s slot as the record keeps it, none where it has none or it cannot
/// be read.
fn kept_slot(mob: &Mob, kind: MemoryKind) -> Option<Slot> {
    let id = crate::mobs::brain::ids::FIRST_MEMORY + kind.id();
    mob.mind
        .brain
        .iter()
        .find(|entry| entry.id == id)
        .and_then(|entry| postcard::from_bytes(&entry.bytes).ok())
}

/// Whether `ValidatePlayDead` ends playing dead in the brain's tick at `now`: its
/// count reads nothing or less as its turn comes, and `EraseMemoryIf` of the play-dead
/// activity, which comes first, has not forgotten it for a breeding. Read from the
/// record before the tick, as nothing in the tick writes the count or the breed target
/// before that turn (a breed target has no expiry, and the count only its lowering).
pub(crate) fn validation_ends(mob: &Mob, now: u32) -> bool {
    let Some(Slot::Count(count)) = kept_slot(mob, MemoryKind::PlayDeadTicks) else {
        return false;
    };
    let active: crate::mobs::brain::BrainState =
        crate::mobs::part::decode(&mob.mind.brain).unwrap_or_default();
    let erased_first = active.active == Some(Activity::PlayDead)
        && kept_slot(mob, MemoryKind::BreedTarget).is_some();
    count.at(now, false) <= 0 && !erased_first
}

/// `AxolotlAi.updateActivity(axolotl)` after the brain's tick, `ended` where
/// `ValidatePlayDead` ended playing dead in it (its `useDefaultActivity`, see the
/// module's comment): out of playing dead, the first of playing dead, fighting and idling
/// whose requirements are met, and a fight that ends leaves a hunting cooldown of 2400.
pub(crate) fn update_activity(brain: &mut Brain, ended: bool) {
    if ended {
        brain.set_active(Activity::Idle);
    }
    let before = brain.active_non_core();
    if before == Some(Activity::PlayDead) {
        return;
    }
    brain.set_active_to_first_valid(&[Activity::PlayDead, Activity::Fight, Activity::Idle]);
    if before == Some(Activity::Fight) && brain.active_non_core() != Some(Activity::Fight) {
        brain.memories.set_with_expiry(
            MemoryKind::HasHuntingCooldown,
            Value::Bool(true),
            HUNTING_COOLDOWN,
        );
    }
}

/// `customServerAiStep`'s brain: its tick and its activities, then with AI the synced
/// flag of playing dead from `play_dead_ticks` (above nothing).
pub(crate) fn step(tick: &mut MobTick<'_>) {
    let ended = validation_ends(&tick.mob, tick.lived);
    crate::mobs::brain::run(tick, |brain, ai| {
        update_activity(brain, ended);
        if !ai.tick.mob.body.flags.no_ai {
            let playing = brain
                .memories
                .int(MemoryKind::PlayDeadTicks)
                .is_some_and(|ticks| ticks > 0);
            super::axolotl::update(&mut ai.tick.mob, |axolotl| {
                axolotl.playing_dead = playing;
            });
        }
    });
}

/// `brain.setMemory(PLAY_DEAD_TICKS, ticks)` of an axolotl outside its tick, at the
/// moment `lived` of its record (a hurt's): counted from its next brain tick.
pub(crate) fn set_play_dead_ticks(mob: &mut Mob, lived: u32, ticks: i32) {
    let mut memories = crate::mobs::brain::memories_of(&PROVIDER, mob, lived);
    memories.set(MemoryKind::PlayDeadTicks, Value::Int(ticks));
    crate::mobs::brain::write_memories(&PROVIDER, mob, memories);
}
