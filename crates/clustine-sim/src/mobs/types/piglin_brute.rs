//! The piglin brute (`PiglinBrute`, `PiglinBruteAi`, `PiglinBruteSpecificSensor`; the d
//! dump): family E5 of P12 (`docs/groundwork/p-plan.md`, section 18). An
//! `AbstractPiglin` (`piglin.rs`: its immunity, its time in the overworld and its
//! zombification) of 50 health, 7 attack and a follow range of 12, which spawns with a
//! golden axe, remembers its spawn as its home and strolls about it, attacks what it is
//! angry at, its nemesis (a wither skeleton or the wither) and any player it may (none
//! is: every player is creative), retaliates where a piglin would and is heard angry now
//! and then while it fights.
//!
//! Its brain's sensors, its anger and what it is told are the piglin's
//! (`piglin_ai.rs`); a brute's own is here.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack};

use super::piglin::{self, PIGLIN, PIGLIN_BRUTE, read};
use super::piglin_ai::{self, maybe_retaliate};
use super::piglin_behaviours::{StopBeingAngryIfTargetDead, WalkToAttackTarget, living_of_uuid};
use super::{MobType, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::melee_hit;
use crate::mobs::ai::sensing::Looked;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body;
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::brain::behaviours::attacking::{
    MeleeAttack, StartAttacking, StopAttackingIfTargetInvalid,
};
use crate::mobs::brain::behaviours::looking::{LookAt, look_at};
use crate::mobs::brain::behaviours::social_villagers::{interact_with, set_look_and_interact};
use crate::mobs::brain::behaviours::walking::{look_sink, sink, stroll};
use crate::mobs::brain::behaviours::{doors, entity_of, work};
use crate::mobs::brain::sensors::{self, attackable_ignoring_line_of_sight, find_closest_visible};
use crate::mobs::brain::{
    self, Activity, ActivityData, Behaviour, Brain, Cx, DoNothing, Gate, HeldShot, HeldTimed,
    MemoryKind, Provider, Sensor, Status, Value,
};
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// Whether the type is whole, so that `built()` gives it.
pub(crate) const BUILT: bool = true;

/// `PiglinBrute`'s `xpReward`.
const EXPERIENCE: i32 = 20;

/// `ACTIVITY_SOUND_LIKELIHOOD_PER_TICK`.
const ACTIVITY_SOUND_CHANCE: f64 = 0.0125;

const PLAYER: u16 = entity_types::PLAYER as u16;

/// The piglin brute.
pub(crate) struct PiglinBrute;

/// `PiglinBruteAi.findNearestValidAttackTarget(level, brute)` (PiglinBruteAi.txt): what
/// it is angry at, where it may attack it whatever is in the way; else the nearest player
/// it may attack; else its nemesis.
fn nearest_valid_attack_target(cx: &mut Cx<'_, '_, '_>) -> Option<EntityId> {
    if let Some(Value::Uuid(uuid)) = cx.memories.get(MemoryKind::AngryAt)
        && let Some(angry) = living_of_uuid(cx, uuid)
        && attackable_ignoring_line_of_sight(cx, &angry)
    {
        return Some(angry.key);
    }
    if let Some(player) = entity_of(cx, MemoryKind::NearestVisibleTargetablePlayer) {
        return Some(player);
    }
    entity_of(cx, MemoryKind::NearestVisibleNemesis)
}

fn not_nearest_valid_attack_target(cx: &mut Cx<'_, '_, '_>, target: &Looked) -> bool {
    nearest_valid_attack_target(cx) != Some(target.key)
}

fn always(_: &mut Cx<'_, '_, '_>) -> bool {
    true
}

fn any(_: &Cx<'_, '_, '_>) -> bool {
    true
}

fn no_callback(_: &mut Cx<'_, '_, '_>, _: EntityId) {}

fn hit(cx: &mut Cx<'_, '_, '_>, target: &Looked) {
    melee_hit(cx.ai, target);
}

fn timed(min: i64, max: i64) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(DoNothing { min, max }))
}

/// `PiglinBruteAi.getActivities(brute)`: core, idle and fighting.
fn activities() -> Vec<ActivityData> {
    let core = ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            look_sink(45, 90),
            sink(),
            doors::interact_with_door(),
            Box::new(HeldShot::new(StopBeingAngryIfTargetDead)),
        ],
    );
    let idle = ActivityData::numbered(
        Activity::Idle,
        10,
        vec![
            Box::new(HeldShot::new(StartAttacking {
                condition: always,
                finder: nearest_valid_attack_target,
            })),
            Box::new(Gate::run_one(vec![
                (look_at(LookAt::Player, 8.0), 1),
                (look_at(LookAt::Kind(PIGLIN), 8.0), 1),
                (look_at(LookAt::Kind(PIGLIN_BRUTE), 8.0), 1),
                (look_at(LookAt::Any, 8.0), 1),
                (timed(30, 60), 1),
            ])),
            Box::new(Gate::run_one(vec![
                (stroll(0.6), 2),
                (
                    interact_with(PIGLIN, 8, MemoryKind::InteractionTarget, 0.6, 2),
                    2,
                ),
                (
                    interact_with(PIGLIN_BRUTE, 8, MemoryKind::InteractionTarget, 0.6, 2),
                    2,
                ),
                (work::stroll_to_poi(MemoryKind::Home, 0.6, 2, 100), 2),
                (work::stroll_around_poi(MemoryKind::Home, 0.6, 5), 2),
                (timed(30, 60), 1),
            ])),
            set_look_and_interact(PLAYER, 4),
        ],
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
            Box::new(HeldShot::new(WalkToAttackTarget { speed: 1.0 })),
            Box::new(HeldShot::new(MeleeAttack {
                predicate: any,
                cooldown: 20,
                hit,
            })),
        ],
        MemoryKind::AttackTarget,
    );
    vec![core, idle, fight]
}

/// `PIGLIN_BRUTE_SPECIFIC_SENSOR`: its nemesis (the closest wither skeleton or wither it
/// sees) and the adult piglins it lives near.
pub(crate) struct BruteSensor;

pub(crate) static BRUTE_SPECIFIC: BruteSensor = BruteSensor;

impl Sensor for BruteSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[
            MemoryKind::VisibleMobs,
            MemoryKind::NearestVisibleNemesis,
            MemoryKind::NearbyAdultPiglins,
        ]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let nemesis = find_closest_visible(cx, &mut |_, looked| {
            matches!(
                looked.kind.map(i32::from),
                Some(entity_types::WITHER_SKELETON | entity_types::WITHER)
            )
        });
        let nearby = piglin_ai::nearby_adult_piglins(cx);
        match nemesis.filter(|looked| looked.mob) {
            Some(nemesis) => cx.memories.set(
                MemoryKind::NearestVisibleNemesis,
                Value::Entity(nemesis.key),
            ),
            None => cx.memories.erase(MemoryKind::NearestVisibleNemesis),
        }
        cx.memories
            .set(MemoryKind::NearbyAdultPiglins, Value::Entities(nearby));
    }
}

/// `PiglinBrute.BRAIN_PROVIDER`: the adult piglins it sees named; the living, the players,
/// the items, what hurt it and its own sensor.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[MemoryKind::NearestVisibleAdultPiglins],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::NEAREST_PLAYERS,
        &piglin_ai::NEAREST_ITEMS,
        &sensors::HURT_BY,
        &BRUTE_SPECIFIC,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `PiglinBruteAi.wasHurtBy(level, brute, attacker)`: nothing for a piglin or a brute;
/// else `PiglinAi.maybeRetaliate`.
pub(crate) fn was_hurt_by(cx: &mut Cx<'_, '_, '_>, attacker: &Looked) {
    if matches!(attacker.kind, Some(PIGLIN | PIGLIN_BRUTE)) {
        return;
    }
    // A brute never avoids (it has no such activity).
    maybe_retaliate(cx, false, attacker);
}

/// `PiglinBruteAi.updateActivity(brute)`: fighting or idle, a change into a fight heard
/// (`playActivitySound`), aggressive while it has an attack target.
fn update_activity(brain: &mut Brain, ai: &mut Ai<'_, '_>) {
    let before = brain.active_non_core();
    brain.set_active_to_first_valid(&[Activity::Fight, Activity::Idle]);
    let after = brain.active_non_core();
    if before != after {
        play_activity_sound(ai.tick, after);
    }
    ai.tick.mob.body.combat.aggressive = brain
        .memories
        .check(MemoryKind::AttackTarget, Status::Present);
}

/// `playActivitySound(brute)`: `playAngrySound` while fighting.
fn play_activity_sound(tick: &mut MobTick<'_>, activity: Option<Activity>) {
    if activity == Some(Activity::Fight) {
        let volume = tick.mob_type.sound_volume_of(&tick.mob);
        body::make_sound(tick, Some(sounds::ENTITY_PIGLIN_BRUTE_ANGRY), volume);
    }
}

/// `PiglinBruteAi.initMemories(brute)`: its home, where it is made.
fn init_memories(mob: &mut Mob, at: BlockPos) {
    let mut memories = brain::memories_of(&PROVIDER, mob, 0);
    memories.set(MemoryKind::Home, Value::Block(at));
    brain::write_memories(&PROVIDER, mob, memories);
}

/// The brute's piglin conversion has nothing of its own to drop.
fn finish_conversion(_: &mut MobTick<'_>) {}

impl MobType for PiglinBrute {
    fn register_goals(&self, setup: &mut Setup) {
        piglin::register_abstract(setup);
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `setCanPickUpLoot(true)`.
    fn construct(&self, mob: &mut Mob, _: &mut Random) {
        mob.body.flags.picks_up_loot = true;
    }

    /// `PiglinBrute.customServerAiStep`: what it was told or hurt by, its brain's tick,
    /// its activity's update, now and then its activity's sound (`nextFloat() < 0.0125`
    /// of the level's), then `AbstractPiglin`'s conversion.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        piglin_ai::take_pending(tick);
        brain::run(tick, update_activity);
        if f64::from(tick.draws.next_float()) < ACTIVITY_SOUND_CHANCE {
            let activity = piglin_ai::recorded_activity(&tick.mob);
            play_activity_sound(tick, Some(activity));
        }
        piglin::zombify_step(
            tick,
            sounds::ENTITY_PIGLIN_BRUTE_CONVERTED_TO_ZOMBIFIED,
            finish_conversion,
        );
    }

    /// `PiglinBrute.finalizeSpawn`: its home, then its golden axe
    /// (`populateDefaultEquipmentSlots`, no draw). `Mob`'s follows.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        init_memories(spawn.mob, spawn.at);
        spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
            Some(ItemStack::new(items::GOLDEN_AXE, 1));
    }

    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        piglin::from_template_abstract(mob, tag);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        super::monster_ai::walk_target_value(ai, pos)
    }

    /// `Mob.aiStep`'s pick-up of a golden axe.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        piglin_ai::pick_up(tick);
    }

    /// `PiglinBrute.hurtServer`: `wasHurtBy`, kept for its next AI step.
    fn after_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
        piglin::note_hurt(struck, source, took);
    }

    fn told(&self, mob: &mut Mob, told: &clustine_world::Told, seen: bool, tick: u64) -> bool {
        let _ = tick;
        piglin_ai::told(mob, told, seen)
    }

    /// `Mob.getBaseExperienceReward` of its `xpReward` of 20.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        piglin::experience_with_equipment(EXPERIENCE, mob, draws)
    }

    /// `AbstractPiglin.playAmbientSound()`: only while idle.
    fn play_ambient_sound(&self, tick: &mut MobTick<'_>) {
        if piglin_ai::is_idle(&tick.mob) {
            body::play_ambient_sound(tick);
        }
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PIGLIN_BRUTE_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_PIGLIN_BRUTE_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PIGLIN_BRUTE_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PIGLIN_BRUTE_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        vec![piglin::shown_abstract(&read(mob))]
    }
}
