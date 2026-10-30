//! The hoglin (`Hoglin`; d Hoglin.txt, HoglinAi.txt, HoglinBase.txt; the brief's 5.7):
//! family E6 of P12 (`docs/groundwork/p-plan.md`, sections 18.3, 18.5 and 18.6).
//!
//! An animal of the class `Animal` that is an `Enemy`, bred with crimson fungi
//! (`#hoglin_food`), whose mind is its brain ([`super::hoglin_ai`]): it fights what hurts
//! it and what the hoglins about it fight, throws what it hits, flees piglins that
//! outnumber its kind and keeps off warped fungi, portals and respawn anchors. A fifth of
//! those an egg or the spawning makes are babies, whose hit is weaker and throws nothing.
//! Where piglins zombify (the overworld and the End: `piglins_zombify` of the dimension's
//! type, which no biome sets) one that is not immune and has its mind counts its time
//! there, and after 300 ticks becomes a zoglin, with nausea for ten seconds.
//!
//! It keeps `TimeInOverworld`, `IsImmuneToZombification` (synced, index 18) and
//! `CannotBeHunted` (a bastion's stable's hoglins, from the template's tag), and the hurts
//! and words since its last tick that its next answers ([`Pending`]).
//!
//! Named differences: the attack's animation counter is the client's only, set by entity
//! event 4; a shield's `blockedByItem` throw has no caller (no mob blocks); the ambient
//! sound of a hoglin an egg makes is read without the dimension, so one made converting
//! sounds its ambient where the game's retreats.

use clustine_blocks::data::is_block;
use clustine_data::{attributes, blocks, entity_types, sounds};
use clustine_world::BlockPos;
use clustine_world::generation::Draws;
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::hoglin_ai::{self, PROVIDER};
use super::{Breeding, Dimensions, Interaction, MobType, Spawning, Used, template};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::brain::{self as machinery, MemoryKind, Provider, Status};
use crate::mobs::tick::{Local, MobTick};
use crate::region::Random;
use crate::shown::DataShown;
use crate::spawning::rules::SpawnWorld;

/// Whether the type is whole, so that `built()` gives it.
pub(crate) const BUILT: bool = true;

/// `#hoglin_food`: crimson fungi.
pub(crate) const FOOD: Items = Items::Tag("minecraft:hoglin_food");

/// A baby's size (`BABY_DIMENSIONS`, Hoglin.txt 617–635).
pub(crate) const BABY: Dimensions = Dimensions {
    width: 0.75,
    height: 0.85,
    eye_height: 0.625,
};

/// The synced index of `DATA_IMMUNE_TO_ZOMBIFICATION`, after `AgeableMob`'s two.
const IMMUNE_INDEX: u8 = 18;

/// `CONVERSION_TIME`: the ticks of zombifying it outlasts.
pub(crate) const CONVERSION_TIME: i32 = 300;

/// `MAX_HEALTH`'s neighbours that `ageBoundaryReached` sets: an adult's attack and a
/// baby's (`ATTACK_DAMAGE`, `BABY_ATTACK_DAMAGE`).
const ATTACK_DAMAGE: f64 = 6.0;
const BABY_ATTACK_DAMAGE: f64 = 0.5;

/// The nausea a zoglin made of it is given (`lambda$finishConversion$0`).
const NAUSEA_TICKS: i32 = 200;

/// The hoglin's entity type.
const KIND: u16 = entity_types::HOGLIN as u16;

/// What came to a hoglin between its ticks, which its next tick answers
/// (`hoglin_ai::take_pending`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Pending {
    /// A hurt that took, caused by the living thing `key` of the type `kind`.
    HurtBy {
        key: clustine_world::EntityId,
        kind: u16,
    },
    /// Another hoglin's word of the target it took.
    TargetOf(clustine_world::EntityId),
}

/// What a hoglin keeps beyond an animal.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct HoglinState {
    /// `timeInOverworld`.
    pub(crate) time_in_overworld: i32,
    /// `DATA_IMMUNE_TO_ZOMBIFICATION`.
    pub(crate) immune_to_zombification: bool,
    /// `cannotBeHunted`.
    pub(crate) cannot_be_hunted: bool,
    /// The hurts and words since its last tick, in the order they came.
    pub(crate) pending: Vec<Pending>,
}

impl Own for HoglinState {}

/// The hoglin's own state of `mob`.
pub(crate) fn state_of(mob: &Mob) -> HoglinState {
    animal::read::<HoglinState>(&mob.state).1
}

/// Changes the hoglin's own state of `mob`, written only where it changed.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut HoglinState)) {
    let (shared, mut own) = animal::read::<HoglinState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// The most a record keeps of what came between its ticks: a hoglin whose mind does not
/// tick (no AI) answers nothing, and keeps only the last of what came.
pub(crate) const MOST_PENDING: usize = 8;

/// `each` kept after what came before it, the oldest let go past [`MOST_PENDING`].
pub(crate) fn keep<T>(pending: &mut Vec<T>, each: T) {
    if pending.len() >= MOST_PENDING {
        pending.remove(0);
    }
    pending.push(each);
}

/// What came to it since its last tick, taken.
pub(crate) fn take_pending(mob: &mut Mob) -> Vec<Pending> {
    let mut taken = Vec::new();
    update(mob, |own| taken = std::mem::take(&mut own.pending));
    taken
}

/// `Hoglin.isConverting()` (Hoglin.txt 473–494): not immune, with its mind, and where
/// piglins zombify.
pub(crate) fn is_converting(tick: &MobTick<'_>) -> bool {
    !state_of(&tick.mob).immune_to_zombification
        && !tick.mob.body.flags.no_ai
        && tick.world.dimension().rules().piglins_zombify
}

/// `ageBoundaryReached` (Hoglin.txt 232–254): the base of its attack damage a baby's 0.5
/// and an adult's 6, set where the instance has not that value (once a baby is made,
/// once it grows up), before anything of its tick reads it.
fn age_boundary(mob: &mut Mob) {
    let wanted = if animal::is_baby(mob) {
        BABY_ATTACK_DAMAGE
    } else {
        ATTACK_DAMAGE
    };
    let id = crate::mobs::body::attribute(attributes::ATTACK_DAMAGE);
    if mob.body.attributes.value(KIND, id) != wanted
        && let Some(instance) = mob.body.attributes.instance(KIND, id)
    {
        instance.base = wanted;
    }
}

/// `setBaby(true)` of an animal: the baby's start age at its first moment.
fn made_a_baby(mob: &mut Mob) {
    animal::update(mob, |own| own.set_age(animal::BABY_START_AGE, 0));
}

/// `!Hoglin.canBeHunted()` (Hoglin.txt 503–514): a baby, or one a stable keeps
/// (`CannotBeHunted`).
pub(crate) fn cannot_be_hunted(mob: &Mob) -> bool {
    animal::is_baby(mob) || state_of(mob).cannot_be_hunted
}

/// `HoglinAi.isPacified` of a hoglin outside its tick.
fn pacified(mob: &Mob, lived: u32) -> bool {
    machinery::memories_of(&PROVIDER, mob, lived).check(MemoryKind::Pacified, Status::Present)
}

/// `Hoglin.checkHoglinSpawnRules` (Hoglin.txt 256–268): anywhere but above a nether wart
/// block.
pub(crate) fn check_spawn_rules(
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    let _ = draws;
    !world
        .block(BlockPos::new(pos.x, pos.y - 1, pos.z))
        .is_some_and(|state| is_block(state, blocks::NETHER_WART_BLOCK))
}

/// The hoglin.
pub(crate) struct Hoglin;

impl MobType for Hoglin {
    /// `Animal`'s maluses; its goals are its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        super::cow::animal_maluses(setup);
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `Hoglin.getWalkTargetValue` (Hoglin.txt 294–312): −1 within 8 of the repellent it
    /// remembers, 10 over crimson nylium, else 0. **Named**: the repellent is the one the
    /// region keeps beside its record, as its brain's tick began.
    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        if hoglin_ai::near_kept_repellent(ai.tick.sensed.as_ref(), pos) {
            return -1.0;
        }
        let nylium = ai
            .tick
            .world
            .block(BlockPos::new(pos.x, pos.y - 1, pos.z))
            .is_some_and(|state| is_block(state, blocks::CRIMSON_NYLIUM));
        if nylium { 10.0 } else { 0.0 }
    }

    /// `Hoglin.customServerAiStep` (Hoglin.txt 165–203), with no call to `Animal`'s: what
    /// came since its last tick answered ([`hoglin_ai::take_pending`]), its brain's tick
    /// and `updateActivity`, then its zombification counted, and past 300 ticks its
    /// converted sound and the zoglin. Before them its attack's base as its age has it.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        age_boundary(&mut tick.mob);
        hoglin_ai::take_pending(tick);
        machinery::run(tick, hoglin_ai::update_activity);
        if !is_converting(tick) {
            update(&mut tick.mob, |own| own.time_in_overworld = 0);
            return;
        }
        let mut time = 0;
        update(&mut tick.mob, |own| {
            own.time_in_overworld = own.time_in_overworld.wrapping_add(1);
            time = own.time_in_overworld;
        });
        if time > CONVERSION_TIME {
            crate::mobs::body::make_sound(
                tick,
                Some(sounds::ENTITY_HOGLIN_CONVERTED_TO_ZOMBIFIED),
                1.0,
            );
            // `finishConversion`: `convertTo(ZOGLIN, single(this, true, false))`, its
            // equipment kept, and nausea for 200 ticks given the zoglin after.
            tick.out.local.push(Local::Convert {
                into: entity_types::ZOGLIN as u16,
                keep_equipment: true,
                sound: None,
                after: Some(Box::new(clustine_world::components::EffectInstance {
                    effect: "minecraft:nausea".to_owned(),
                    amplifier: 0,
                    duration: NAUSEA_TICKS,
                    ambient: false,
                    visible: true,
                    show_icon: true,
                })),
            });
        }
    }

    /// `aiStep`: `Animal`'s after `LivingEntity`'s (the attack counter is the client's).
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Hoglin.mobInteract` (Hoglin.txt 314–327): an animal's use, but that a pacified
    /// hoglin falls in love with nothing (`canFallInLove`); and where it did anything,
    /// the hoglin kept from despawning.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let food = FOOD.test(used.held.as_ref());
        let adult = !animal::is_baby(used.mob);
        let result = if food && adult && pacified(used.mob, used.moment) {
            // `Animal.mobInteract` with `canFallInLove` false: an adult's food does
            // nothing, and `AgeableMob`'s use is of no food.
            Interaction::Pass
        } else {
            animal::mob_interact(used, FOOD, None)
        };
        if result != Interaction::Pass {
            used.mob.body.flags.persistent = true;
        }
        result
    }

    /// `Hoglin.finalizeSpawn` (Hoglin.txt 270–287): a baby by one of the level's draws
    /// under 0.2, then `Animal`'s (a later one of a group a baby one time in twenty).
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        if spawn.level.next_float() < 0.2 {
            made_a_baby(spawn.mob);
        }
        animal::finalize_ageable_default(spawn);
    }

    /// `getBreedOffspring` (Hoglin.txt 516–529): a hoglin, kept from despawning.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        let mut child = Mob::fresh();
        child.body.flags.persistent = true;
        Some(child)
    }

    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        if baby {
            made_a_baby(mob);
        } else {
            animal::update(mob, |own| own.set_age(0, 0));
        }
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    /// `Hoglin.hurtServer` (Hoglin.txt 125–149): a hurt that took, of a living cause, is
    /// `HoglinAi.wasHurtBy`, answered at its next tick.
    fn after_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
        if !took {
            return;
        }
        if let Some(cause) = source.cause.as_ref().filter(|cause| cause.living) {
            let pending = Pending::HurtBy {
                key: cause.key,
                kind: cause.kind,
            };
            update(struck.mob, |own| keep(&mut own.pending, pending));
        }
    }

    /// Told by another hoglin of its target (`setAttackTargetIfCloserThanCurrent`),
    /// answered at its next tick.
    fn told(&self, mob: &mut Mob, told: &clustine_world::Told, _: bool, _: u64) -> bool {
        if told.what != clustine_world::ToldWhat::TargetOf {
            return false;
        }
        update(mob, |own| {
            keep(&mut own.pending, Pending::TargetOf(told.of))
        });
        true
    }

    fn cannot_be_hunted(&self, mob: &Mob) -> bool {
        cannot_be_hunted(mob)
    }

    /// `readAdditionalSaveData`'s own (Hoglin.txt 428–451): a bastion's hoglin is one
    /// that cannot be hunted.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let immune = template::boolean(tag, "IsImmuneToZombification").unwrap_or(false);
        let time = template::int(tag, "TimeInOverworld").unwrap_or(0);
        let hunted = template::boolean(tag, "CannotBeHunted").unwrap_or(false);
        update(mob, |own| {
            own.immune_to_zombification = immune;
            own.time_in_overworld = time;
            own.cannot_be_hunted = hunted;
        });
    }

    /// `removeWhenFarAway`: always, unlike an animal's.
    fn remove_when_far_away(&self, _: u16, _: &Mob, _: u32, _: f64) -> bool {
        true
    }

    /// `getBaseExperienceReward`: its `xpReward`, 5, or 3 as a baby (`ageBoundaryReached`),
    /// drawing nothing.
    fn experience_reward(&self, _: u16, mob: &Mob, _: &mut Random) -> i32 {
        if animal::is_baby(mob) { 3 } else { 5 }
    }

    fn can_be_leashed(&self, _: u16, _: &Mob) -> bool {
        true
    }

    /// `getAmbientSound()`: its activity's (`getSoundForCurrentActivity`), read in its
    /// tick with where it is and what the region keeps of its senses.
    fn play_ambient_sound(&self, tick: &mut MobTick<'_>) {
        let activity = hoglin_ai::recorded_activity(&tick.mob);
        let near = hoglin_ai::kept_repellent(tick.sensed.as_ref());
        let sound = hoglin_ai::sound_for_activity(activity, is_converting(tick), near);
        crate::mobs::body::make_sound(tick, Some(sound), 1.0);
    }

    /// Its activity's sound as its record has it, for an egg's: not converting, no
    /// repellent near (named).
    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(hoglin_ai::sound_for_activity(
            hoglin_ai::recorded_activity(mob),
            false,
            false,
        ))
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_HOGLIN_STEP)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_HOGLIN_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_HOGLIN_DEATH)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let immune = state_of(mob).immune_to_zombification;
        let mut values = animal::shown(mob);
        values.push((IMMUNE_INDEX, DataShown::Boolean(immune), !immune));
        values
    }
}
