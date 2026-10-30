//! The zoglin (`Zoglin`; d Zoglin.txt): family E6 of P12 (`docs/groundwork/p-plan.md`,
//! sections 18.3, 18.5 and 18.6).
//!
//! A monster that a hoglin becomes where piglins zombify, and of its egg. Its mind is a
//! brain of two activities: idle, where it takes as its target the nearest living thing
//! it sees and may attack but a creeper or another zoglin, and fight, where it walks to
//! its target and hits it as a hoglin does, throwing it (`HoglinBase`,
//! [`super::hoglin_ai::hurt_and_throw_target`]). It never breeds. A fifth of those an egg
//! or the spawning makes are babies (`IsBaby`, synced at 16), whose hit is weaker and
//! throws nothing.
//!
//! What hurts it becomes its target where it may attack it and it is not much farther
//! than the one it has (`hurtServer`): answered at the head of its next tick, as a
//! hoglin's hurt is ([`super::hoglin_ai`]'s comment says why).
//!
//! Named differences: the attack's animation counter is the client's only, set by entity
//! event 4; a shield's `blockedByItem` throw has no caller.

use clustine_data::{attributes, entity_types, sounds};
use clustine_world::{EntityId, TypeState};
use serde::{Deserialize, Serialize};

use super::hoglin_ai::{self, idle_movement};
use super::{Dimensions, MobType, Spawning, template};
use crate::mobs::Mob;
use crate::mobs::ai::sensing::{self, Looked, Sensing};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::body::{self, Event};
use crate::mobs::brain::behaviours::attacking::{self, MeleeAttack};
use crate::mobs::brain::behaviours::looking::{self, LookAt};
use crate::mobs::brain::behaviours::{Speed, trigger_if};
use crate::mobs::brain::sensors::{self, find};
use crate::mobs::brain::{
    self as machinery, Activity, ActivityData, Behaviour, Brain, Cx, HeldShot, MemoryKind,
    Provider, Status, Value,
};
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// Whether the type is whole, so that `built()` gives it.
pub(crate) const BUILT: bool = true;

/// A baby's size (`BABY_DIMENSIONS`, Zoglin.txt 402–420).
pub(crate) const BABY: Dimensions = Dimensions {
    width: 0.75,
    height: 0.85,
    eye_height: 0.625,
};

/// The synced index of `DATA_BABY_ID`.
const BABY_INDEX: u8 = 16;

/// `BABY_ATTACK_DAMAGE`, which `setBaby(true)` sets as the attack's base.
const BABY_ATTACK_DAMAGE: f64 = 0.5;

/// The zoglin's entity type.
const KIND: u16 = entity_types::ZOGLIN as u16;

/// What a zoglin keeps: whether it is a baby, and the living things that hurt it since
/// its last tick, in the order they came.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ZoglinState {
    pub(crate) baby: bool,
    pub(crate) hurt_by: Vec<EntityId>,
}

/// The version of its type state.
const VERSION: u8 = 1;

/// The zoglin's state of `mob`; a record of nothing, or one this build cannot read, as a
/// fresh zoglin's.
pub(crate) fn state_of(mob: &Mob) -> ZoglinState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return ZoglinState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// Its state changed by `change`, written only where it changed; a fresh one's as
/// nothing.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut ZoglinState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own == before {
        return;
    }
    mob.state = if own == ZoglinState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
        }
    };
}

/// `Zoglin.setBaby(baby)` (Zoglin.txt 284–297): the flag, and for a baby the attack's
/// base 0.5 (never set back).
fn set_baby(mob: &mut Mob, baby: bool) {
    update(mob, |own| own.baby = baby);
    if baby {
        let id = crate::mobs::body::attribute(attributes::ATTACK_DAMAGE);
        if let Some(instance) = mob.body.attributes.instance(KIND, id) {
            instance.base = BABY_ATTACK_DAMAGE;
        }
    }
}

/// `Zoglin.BRAIN_PROVIDER` (Zoglin.txt 421–426): the nearest living things and players.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[],
    sensors: &[&sensors::NEAREST_LIVING_ENTITIES, &sensors::NEAREST_PLAYERS],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `Zoglin.updateActivity`'s list.
const ACTIVITIES: [Activity; 2] = [Activity::Fight, Activity::Idle];

/// `Zoglin.getActivities()` (Zoglin.txt 44–124).
fn activities() -> Vec<ActivityData> {
    let core = ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            crate::mobs::brain::behaviours::walking::look_sink(45, 90),
            crate::mobs::brain::behaviours::walking::sink(),
        ],
    );
    let idle = ActivityData::numbered(
        Activity::Idle,
        10,
        vec![
            attacking::start_attacking(find_nearest_valid_attack_target),
            looking::look_sometimes(LookAt::Any, 8.0, (30, 60)),
            idle_movement(),
        ],
    );
    let fight = ActivityData::while_present(
        Activity::Fight,
        10,
        vec![
            attacking::walk_to_attack_target(Speed::Fixed(1.0)),
            trigger_if(is_adult, melee(hoglin_ai::ATTACK_INTERVAL)),
            trigger_if(is_baby, melee(hoglin_ai::BABY_ATTACK_INTERVAL)),
            attacking::stop_attacking(),
        ],
        MemoryKind::AttackTarget,
    );
    vec![core, idle, fight]
}

fn is_baby(cx: &mut Cx<'_, '_, '_>) -> bool {
    state_of(&cx.ai.tick.mob).baby
}

fn is_adult(cx: &mut Cx<'_, '_, '_>) -> bool {
    !is_baby(cx)
}

/// `MeleeAttack.create(cooldown)` whose hit is the zoglin's `doHurtTarget`.
fn melee(cooldown: i64) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(MeleeAttack {
        predicate: |_| true,
        cooldown,
        hit: zoglin_hits,
    }))
}

/// `Zoglin.findNearestValidAttackTarget` (Zoglin.txt 126–135, 384–397): the nearest
/// living thing it sees that is neither a zoglin nor a creeper and that it may attack.
fn find_nearest_valid_attack_target(cx: &mut Cx<'_, '_, '_>) -> Option<EntityId> {
    sensors::find_closest_visible(cx, &mut |cx, looked| {
        let kind = looked.kind.map(i32::from);
        kind != Some(entity_types::ZOGLIN)
            && kind != Some(entity_types::CREEPER)
            && sensors::attackable(cx, looked)
    })
    .map(|looked| looked.key)
}

/// `Zoglin.doHurtTarget` (Zoglin.txt 191–204): its arms shown (entity event 4), its
/// attack's sound, and the hurt that throws.
fn zoglin_hits(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    if !sensors::is_living(target) {
        return;
    }
    cx.ai.tick.out.events.push(Event::Entity(4));
    body::make_sound(cx.ai.tick, Some(sounds::ENTITY_ZOGLIN_ATTACK), 1.0);
    let baby = state_of(&cx.ai.tick.mob).baby;
    hoglin_ai::hurt_and_throw_target(cx.ai, target, baby);
}

/// `Zoglin.updateActivity` (Zoglin.txt 247–272): fight or idle; its anger heard as it
/// begins to fight; its aggression as an attack target is remembered.
fn update_activity(brain: &mut Brain, ai: &mut Ai<'_, '_>) {
    let old = brain.active_non_core();
    brain.set_active_to_first_valid(&ACTIVITIES);
    let new = brain.active_non_core();
    if new == Some(Activity::Fight) && old != Some(Activity::Fight) {
        body::make_sound(ai.tick, Some(sounds::ENTITY_ZOGLIN_ANGRY), 1.0);
    }
    ai.tick.mob.body.combat.aggressive = brain
        .memories
        .check(MemoryKind::AttackTarget, Status::Present);
}

/// `Zoglin.hurtServer`'s answer at the head of its next tick (Zoglin.txt 278–313): each
/// living thing that hurt it, where it may attack it (`canAttack`) and it is not much
/// farther than its target, becomes its target (`setAttackTarget`: no unreachable walk,
/// the target for 200 ticks).
fn take_hurts(tick: &mut MobTick<'_>) {
    let mut hurts = Vec::new();
    update(&mut tick.mob, |own| {
        hurts = std::mem::take(&mut own.hurt_by)
    });
    if hurts.is_empty() {
        return;
    }
    let lived = tick.lived;
    let time = tick.world.game_time();
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
        let cx = Cx {
            ai: &mut ai,
            memories: &mut memories,
            time,
        };
        for key in hurts {
            let Some(attacker) = find(cx.ai, key).filter(sensors::is_living) else {
                continue;
            };
            if !sensing::can_attack(&attacker)
                || hoglin_ai::much_further_than_target(&cx, &attacker)
            {
                continue;
            }
            cx.memories.erase(MemoryKind::CantReachWalkTargetSince);
            cx.memories.set_with_expiry(
                MemoryKind::AttackTarget,
                Value::Entity(attacker.key),
                hoglin_ai::ATTACK_DURATION,
            );
        }
    }
    machinery::write_memories(&PROVIDER, &mut tick.mob, memories);
}

/// The zoglin.
pub(crate) struct Zoglin;

impl MobType for Zoglin {
    fn register_goals(&self, _: &mut Setup) {}

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `Zoglin.customServerAiStep`: its brain's tick and `updateActivity`, after the
    /// hurts since its last tick are answered.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        take_hurts(tick);
        machinery::run(tick, update_activity);
    }

    /// `Zoglin.finalizeSpawn` (Zoglin.txt 153–162): a baby by one of the level's draws
    /// under 0.2, then `Monster`'s.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        if spawn.level.next_float() < 0.2 {
            set_baby(spawn.mob, true);
        }
    }

    /// A hoglin's baby converted is a baby zoglin.
    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        set_baby(mob, baby);
    }

    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        set_baby(mob, template::boolean(tag, "IsBaby").unwrap_or(false));
    }

    /// A hurt that took, of a living cause, answered at its next tick.
    fn after_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
        if !took {
            return;
        }
        if let Some(cause) = source.cause.as_ref().filter(|cause| cause.living) {
            let key = cause.key;
            update(struck.mob, |own| super::hoglin::keep(&mut own.hurt_by, key));
        }
    }

    /// `xpReward` 5, with what its equipment adds (`Mob.getBaseExperienceReward`).
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(5, mob, draws)
    }

    fn can_be_leashed(&self, _: u16, _: &Mob) -> bool {
        true
    }

    /// `getAmbientSound()`: its anger with an attack target, else its ambient.
    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        let memories = machinery::memories_of(&PROVIDER, mob, 0);
        Some(
            if memories.check(MemoryKind::AttackTarget, Status::Present) {
                sounds::ENTITY_ZOGLIN_ANGRY
            } else {
                sounds::ENTITY_ZOGLIN_AMBIENT
            },
        )
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOGLIN_STEP)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ZOGLIN_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOGLIN_DEATH)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        state_of(mob).baby.then_some(BABY)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        state_of(mob).baby
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let baby = state_of(mob).baby;
        vec![(BABY_INDEX, DataShown::Boolean(baby), !baby)]
    }
}
