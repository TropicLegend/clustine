//! The zombified piglin (`ZombifiedPiglin` on `Zombie`, a `NeutralMob`;
//! `docs/groundwork/p-plan.md`, sections 18.3, 18.5 and 18.6, and
//! `docs/groundwork/p-brief.md`, section 5.8; d ZombifiedPiglin): a zombie of the
//! family's kind ([`super::zombie_family`]) that is neutral until angered.
//!
//! What is its own (ZombifiedPiglin.txt):
//! - its attributes, the data's: speed 0.23, attack 5, no reinforcements, and none drawn
//!   for it (`createAttributes` 108–121, `randomizeReinforcementsChance` 426–434);
//! - lava a malus of 8 to its paths (the constructor, 25–36);
//! - its goals (`addBehaviourGoals` 37–107): the spear's slot (1, as the family's, never
//!   started), `ZombieAttackGoal(1.0, false)` (2), the stroll avoiding water (7); its
//!   targets: who hurt it, alerting its own kind (1), players it is angry at (2, interval
//!   10, seen), and the universal anger's reset (3, only with `universal_anger`, off);
//! - its anger (`NeutralMob`, [`super::neutral`]): 20 to 39 seconds from the last tick
//!   it held a target (`startPersistentAngerTimer` 302–312), 0.05 faster while angry
//!   unless a baby (`customServerAiStep` 139–182), its angry sound once a moment after
//!   it took a target (`maybePlayFirstAngerSound` 183–200, 263–277) and its ambient
//!   sound the angry one while angry (389–398);
//! - every 4 to 6 seconds while it has a target it sees, the others of its kind about
//!   it with no target take its target (`maybeAlertOthers` 201–228, `alertOthers`
//!   229–262: the unit cube at its feet inflated by its follow range across and 10 up
//!   and down);
//! - a golden sword at its making, or a golden spear one time in 20, and no armour
//!   (`populateDefaultEquipmentSlots` 409–425, which does not call `Mob`'s);
//! - a baby's eyes at 0.78 (`BABY_DIMENSIONS`, 496–510);
//! - no drowning (`convertsInWater` 134–138);
//! - its spawn rule: anything but peaceful, on anything but a nether wart block
//!   (`checkZombifiedPiglinSpawnRules` 313–330).
//!
//! It is immune to fire (its type's `fireImmune`) and does not burn by day (it is not
//! in `#burn_in_daylight`).
//!
//! Its type state is the family's, followed by its own ([`PiglinOwn`]): the family's
//! code reads the family's part of it as it reads a zombie's (postcard reads a value
//! from the front of its bytes), and every call into the family's code that writes the
//! state is made through [`keeping_own`], which puts its own part back after.
//!
//! Named differences: `setTarget`'s two draws (the first anger sound's delay and the
//! alert's interval) are made at its next `customServerAiStep` after it came to hold a
//! target, from its own stream there, where the game draws them within the `setTarget`
//! (in its target goal's start, or another's alert): a target taken and dropped within
//! one step draws nothing; `isPreventingPlayerRest` is not asked (no bed asks a mob);
//! `wantsToPickUp` is the family's (nothing at easy); a baby's riding offset
//! (`BABY_DIMENSIONS`' attachment) is the family's.

use clustine_blocks::data::is_block;
use clustine_data::{PathType, attributes as ids, blocks, entity_types, items, sounds};
use clustine_world::generation::Draws;
use clustine_world::{BlockPos, Difficulty, EntityId, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::neutral::{self, Anger};
use super::tamable::Idle;
use super::zombie_family::{self as family, ZombieAttackGoal};
use super::{Breeding, Dimensions, MobType, Spawning, polar_bear};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::RandomStrollGoal;
use crate::mobs::ai::selector::flags;
use crate::mobs::ai::sensing::{self, Looked};
use crate::mobs::ai::targets::{self, Among, NearestTarget, Target};
use crate::mobs::ai::{Ai, Config, Setup};
use crate::mobs::body::attributes::ADD_VALUE;
use crate::mobs::body::hurt::{DamageSource, mob_voice_pitch};
use crate::mobs::body::{Event, Modifier, attribute};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::region::Random;
use crate::shown::DataShown;
use crate::spawning::rules::SpawnWorld;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

const KIND: u16 = entity_types::ZOMBIFIED_PIGLIN as u16;

/// `SPEED_MODIFIER_ATTACKING`: 0.05 faster while angry (ZombifiedPiglin.txt 511–520).
const ATTACKING: &str = "minecraft:attacking";
const ATTACKING_SPEED: f64 = 0.05;

/// What the zombified piglin keeps beyond the family's: its anger, `playFirstAngerSoundIn`,
/// `ticksUntilNextAlert`, and whether it held a target at its last step (which its
/// `setTarget`'s draws are made by).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PiglinOwn {
    pub(crate) anger: Anger,
    pub(crate) first_anger_sound_in: i32,
    pub(crate) until_next_alert: i32,
    pub(crate) had_target: bool,
}

/// Its own part of `mob`'s type state: what follows the family's; a fresh one where
/// nothing does or this build cannot read it.
pub(crate) fn own_of(mob: &Mob) -> PiglinOwn {
    if mob.state.bytes.is_empty() || mob.state.version != family::VERSION {
        return PiglinOwn::default();
    }
    postcard::take_from_bytes::<family::ZombieState>(&mob.state.bytes)
        .ok()
        .and_then(|(_, rest)| postcard::from_bytes(rest).ok())
        .unwrap_or_default()
}

/// `mob`'s type state made the family's part as it is now, followed by `own`: the
/// family's own encoding alone where `own` is a fresh one's.
pub(crate) fn write_own(mob: &mut Mob, own: &PiglinOwn) {
    let zombie = family::read(mob);
    family::write(mob, &zombie);
    if *own == PiglinOwn::default() {
        return;
    }
    let mut bytes = postcard::to_allocvec(&zombie).expect("a type state is encoded into memory");
    bytes.extend(postcard::to_allocvec(own).expect("a type state is encoded into memory"));
    mob.state = TypeState {
        version: family::VERSION,
        bytes,
    };
}

/// `f`, a call into the family's code that may write `mob`'s state (which writes the
/// family's part alone), with its own part put back after.
fn keeping_own<R>(mob: &mut Mob, f: impl FnOnce(&mut Mob) -> R) -> R {
    let own = own_of(mob);
    let result = f(mob);
    write_own(mob, &own);
    result
}

/// `NeutralMob.isAngryAt(entity)` as its players' target goal's selector
/// (`addBehaviourGoals`' lambda): one it may attack that its anger is at.
fn angry_at(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    own_of(&ai.tick.mob).anger.is_angry_at(looked)
}

/// `ZombifiedPiglin.addBehaviourGoals`' targets (ZombifiedPiglin.txt 57–106): who hurt
/// it, alerting its own kind and ignoring nothing; players it is angry at.
const TARGETS: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: &[],
            alert: true,
        },
    ),
    (
        2,
        Target::Nearest(NearestTarget {
            selector: Some(angry_at),
            ..NearestTarget::new(Among::Players, true)
        }),
    ),
];

/// `Zombie.registerGoals` with the zombified piglin's `addBehaviourGoals`
/// (ZombifiedPiglin.txt 25–107).
fn register_goals(setup: &mut Setup) {
    setup.set_malus(PathType::Lava, 8.0);
    family::register_base_goals(setup);
    // `SpearUseGoal`, decided out for mobs as the family's (its slot never starts).
    setup.add_goal(
        1,
        Idle {
            flags: flags::MOVE | flags::LOOK,
        },
    );
    setup.add_goal(2, ZombieAttackGoal::new(1.0, false));
    setup.add_goal(7, RandomStrollGoal::avoiding_water(1.0));
    targets::register(setup, TARGETS);
    // `ResetUniversalAngerTargetGoal(true)`: only with `universal_anger`, which is off.
    setup.add_target(3, Idle { flags: 0 });
}

/// `populateDefaultEquipmentSlots` (ZombifiedPiglin.txt 409–425): a golden spear one
/// time in 20 of the level's random, else a golden sword, in the main hand; no armour.
fn equipment(spawn: &mut Spawning<'_>, _: f32) {
    let item = if spawn.level.next_int(20) == 0 {
        items::GOLDEN_SPEAR
    } else {
        items::GOLDEN_SWORD
    };
    spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = Some(ItemStack::new(item, 1));
}

/// `UniformInt.sample` of `TimeUtil.rangeOfSeconds(low, high)`: `nextInt` over its
/// ticks, one draw.
fn seconds_between(draws: &mut Random, low: i32, high: i32) -> i32 {
    let span = u32::try_from((high - low) * 20 + 1).expect("a range of seconds in order");
    low * 20 + draws.next_int(span) as i32
}

/// The angry sound (`playAngerSound`, ZombifiedPiglin.txt 263–277): at twice its volume
/// and 1.8 times its voice's pitch, two floats of its own.
fn play_anger_sound(tick: &mut MobTick<'_>) {
    let pitch = mob_voice_pitch(tick.mob_type, &tick.mob, tick.draws) * 1.8;
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_ZOMBIFIED_PIGLIN_ANGRY,
        volume: 2.0,
        pitch,
    });
}

/// `alertOthers()` (ZombifiedPiglin.txt 229–262): every other zombified piglin in the
/// unit cube at its feet inflated by its follow range across and 10 up and down that
/// has no target is given its target, in the view's order (`Local::Alert`, which the
/// region applies by the same tests and sends to one in a copy). Teams being none, no
/// one is allied to the target.
fn alert_others(ai: &mut Ai<'_, '_>, target: EntityId) {
    let kind = sensing::find(ai, target)
        .and_then(|found| found.kind)
        .unwrap_or(entity_types::PLAYER as u16);
    let follow = ai.attribute(ids::FOLLOW_RANGE);
    let [x, y, z] = ai.position();
    let bounds = Aabb::new([x, y, z], [x + 1.0, y + 1.0, z + 1.0]).inflate([follow, 10.0, follow]);
    let mut alerted = Vec::new();
    for seen in ai.tick.world.entities_in(&bounds) {
        let entity = seen.entity;
        if entity.key == ai.tick.key || entity.kind != KIND {
            continue;
        }
        if !Mob::decode(&entity.data).is_ok_and(|mob| mob.body.combat.target.is_none()) {
            continue;
        }
        alerted.push(entity.key);
    }
    for key in alerted {
        ai.tick.out.local.push(Local::Alert {
            key,
            attacker: target,
            kind,
            class: KIND,
        });
    }
}

/// `maybeAlertOthers()` (ZombifiedPiglin.txt 201–228) of one holding `target`: its
/// countdown, and at its end an alert where it sees the target and a new interval of 4
/// to 6 seconds.
fn maybe_alert_others(tick: &mut MobTick<'_>, own: &mut PiglinOwn, target: EntityId) {
    if own.until_next_alert > 0 {
        own.until_next_alert -= 1;
        return;
    }
    let config = Config::default();
    let mut ai = Ai {
        tick,
        config: &config,
        sensing: sensing::Sensing::default(),
        navigation_ticked: true,
    };
    let seen = sensing::find(&ai, target)
        .is_some_and(|looked| sensing::has_line_of_sight(&mut ai, &looked));
    if seen {
        alert_others(&mut ai, target);
    }
    own.until_next_alert = seconds_between(ai.tick.draws, 4, 6);
}

/// `ZombifiedPiglin.customServerAiStep` (ZombifiedPiglin.txt 139–182), after the draws
/// of a `setTarget` it came to hold a target by (`setTarget` 278–301): while angry, 0.05
/// faster unless a baby, and its first angry sound counted down; else not faster; its
/// anger brought up to date, its timer drawn again (`updatePersistentAnger(level,
/// true)`); and with a target, its alert counted down.
fn custom_server_ai_step(tick: &mut MobTick<'_>) {
    let now = tick.world.game_time();
    let mut own = own_of(&tick.mob);
    let before = own.clone();
    if tick.mob.body.combat.target.is_some() && !own.had_target {
        own.first_anger_sound_in = seconds_between(tick.draws, 0, 1);
        own.until_next_alert = seconds_between(tick.draws, 4, 6);
    }
    let angry = own.anger.is_angry(now);
    let baby = family::is_baby(&tick.mob);
    if let Some(speed) = tick
        .mob
        .body
        .attributes
        .instance(KIND, attribute(ids::MOVEMENT_SPEED))
    {
        if angry {
            if !baby && !speed.has_modifier(ATTACKING) {
                speed.add_modifier(Modifier {
                    id: ATTACKING.to_owned(),
                    amount: ATTACKING_SPEED,
                    operation: ADD_VALUE,
                });
            }
        } else {
            speed.remove_modifier(ATTACKING);
        }
    }
    if angry && own.first_anger_sound_in > 0 {
        own.first_anger_sound_in -= 1;
        if own.first_anger_sound_in == 0 {
            play_anger_sound(tick);
        }
    }
    neutral::update_persistent_anger(
        &mut tick.mob,
        &mut own.anger,
        tick.world,
        now,
        true,
        tick.draws,
        polar_bear::anger_time,
    );
    if let Some(target) = tick.mob.body.combat.target {
        maybe_alert_others(tick, &mut own, target);
    }
    own.had_target = tick.mob.body.combat.target.is_some();
    if own != before {
        write_own(&mut tick.mob, &own);
    }
}

/// The zombified piglin's spawn rule at a natural reason (`checkZombifiedPiglinSpawnRules`,
/// ZombifiedPiglin.txt 313–330): anything but peaceful, and not on a nether wart block;
/// nothing drawn.
pub(crate) fn check_spawn_rules(
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    let _ = draws;
    Difficulty::WORLD != Difficulty::Peaceful
        && !world
            .block(pos.offset(0, -1, 0))
            .is_some_and(|below| is_block(below, blocks::NETHER_WART_BLOCK))
}

/// The zombified piglin.
pub(crate) struct ZombifiedPiglin;

impl MobType for ZombifiedPiglin {
    /// `Zombie.doHurtTarget`'s fire.
    fn hit_extras(&self, ai: &Ai<'_, '_>) -> Vec<clustine_world::HitExtra> {
        family::hit_extras(ai)
    }

    fn register_goals(&self, setup: &mut Setup) {
        register_goals(setup);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        family::walk_target_value(ai, pos)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        custom_server_ai_step(tick);
    }

    /// `Zombie.finalizeSpawn` with its own equipment, and `handleAttributes`, which
    /// draws no reinforcements' chance for it.
    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let own = own_of(spawn.mob);
        family::finalize_spawn(KIND, spawn, equipment);
        write_own(spawn.mob, &own);
    }

    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        keeping_own(mob, |mob| family::set_baby(KIND, mob, baby));
    }

    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        // `readPersistentAngerSaveData` too: no structure's template keeps an anger.
        keeping_own(mob, |mob| family::from_template(KIND, mob, tag));
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        family::offspring_from_egg(breeding)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        family::dimensions(mob, 0.78)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        family::is_baby(mob)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        family::experience_reward(mob, draws)
    }

    /// `getAmbientSound()`: the angry one while angry.
    fn ambient_sound_drawn(
        &self,
        mob: &Mob,
        _: &mut Random,
        now: i64,
        _: u32,
        _: Option<(bool, bool)>,
    ) -> Option<i32> {
        Some(if own_of(mob).anger.is_angry(now) {
            sounds::ENTITY_ZOMBIFIED_PIGLIN_ANGRY
        } else {
            sounds::ENTITY_ZOMBIFIED_PIGLIN_AMBIENT
        })
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIFIED_PIGLIN_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIFIED_PIGLIN_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIFIED_PIGLIN_DEATH)
    }

    /// `Zombie.getStepSound`, which it does not change.
    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ZOMBIE_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        family::shown(mob)
    }
}

#[cfg(test)]
#[path = "nether_e1_tests.rs"]
mod nether_e1_tests;
