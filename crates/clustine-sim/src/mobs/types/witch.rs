//! The witch (`Witch`; `n7k-brief.md` 3.6, `docs/groundwork/n9-plan.md` 8.4), part K5
//! and N9's S7: a raider that floats, strolls and looks, and in its `aiStep` drinks a
//! potion where it needs one (water breathing under water, fire resistance while it
//! burns, healing when hurt, swiftness after a target beyond 11 blocks), with the game's
//! draws in the game's order; to a target it throws splash potions every three seconds
//! (`RangedAttackGoal(1.0, 60, 10)`, [`perform_ranged_attack`]).
//!
//! Named differences: the potion in its hand is a plain potion (`n7k-plan.md`, section
//! 10): the client draws an empty bottle, and the potion it drinks is kept in its type
//! state; a player it aims at is read as of full health, with no effects and still,
//! which no witch meets, every player being creative and so no target.

use clustine_data::{attributes as ids, entity_types, items, sounds};
use clustine_world::ItemStack;
use clustine_world::components::{Component, PotionContents};

use serde::{Deserialize, Serialize};

use super::illagers;
use super::{MobType, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, LookAtPlayerGoal, RandomLookAroundGoal, RandomStrollGoal, RangedAttackGoal,
};
use crate::mobs::ai::sensing::{self, Class, Looked};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::attributes::ADD_VALUE;
use crate::mobs::body::effects::{self, Effect};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::body::{Event, Modifier, attribute};
use crate::mobs::tick::{Local, MobTick, Shot};
use crate::region::Random;
use crate::shown::DataShown;
use clustine_world::BlockPos;

/// The synced data's index of `DATA_USING_ITEM`.
pub(crate) const USING_ITEM_INDEX: u8 = 17;

/// `SPEED_MODIFIER_DRINKING_ID`.
pub(crate) const DRINKING: &str = "minecraft:drinking";

/// `ItemStack.getUseDuration` of a potion: its `consumable`'s 1.6 seconds.
pub(crate) const POTION_USE_TICKS: i32 = 32;

/// The witch's entity event of its particles (`handleEntityEvent(15)`).
pub(crate) const PARTICLES_EVENT: u8 = 15;

/// The witch.
pub(crate) struct Witch;

/// The potions a witch drinks (`Potions`), by what each gives: water breathing,
/// fire resistance and swiftness for 3600 ticks, healing for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Potion {
    WaterBreathing,
    FireResistance,
    Healing,
    Swiftness,
}

/// The witch's own part of its raider state (`crate::mobs::types::illagers`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct WitchState {
    /// The witch's potion while it drinks (`DATA_USING_ITEM`, synced at 17): what the
    /// potion in its hand would carry, which the stack cannot.
    pub(crate) drinking: Option<Potion>,
    /// The witch's `usingTime`: the drinking's ticks left.
    pub(crate) using_time: i32,
}

impl Potion {
    /// `Potions.*`'s effects: each effect with its amplifier and duration.
    pub(crate) fn effects(self) -> [(Effect, u8, i32); 1] {
        match self {
            Self::WaterBreathing => [(Effect::WATER_BREATHING, 0, 3600)],
            Self::FireResistance => [(Effect::FIRE_RESISTANCE, 0, 3600)],
            // An instant effect of one tick: it heals at the next `tickEffects`.
            Self::Healing => [(Effect::INSTANT_HEALTH, 0, 1)],
            Self::Swiftness => [(Effect::SPEED, 0, 3600)],
        }
    }
}

/// The end of a drink (`usingTime` run out): the hand emptied, the potion's effects
/// added at the moment `now`, the game event `DRINK`, and the drinking's slowness off.
fn finish_drinking(tick: &mut MobTick<'_>, potion: Potion, now: u32) {
    tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = None;
    let kind = tick.kind;
    // `PotionContents.forEachEffect(this::addEffect, 1)`: a potion of no duration
    // scale, each effect as the potion has it.
    for (effect, amplifier, duration) in potion.effects() {
        effects::add(&mut tick.mob, kind, effect, amplifier, duration, now);
    }
    tick.out.local.push(Local::GameEventAt {
        name: "minecraft:drink",
        at: tick.living.body.position,
        affected: None,
    });
    if let Some(speed) = tick
        .mob
        .body
        .attributes
        .instance(kind, attribute(ids::MOVEMENT_SPEED))
    {
        speed.remove_modifier(DRINKING);
    }
}

/// The potion the witch takes up, if any: `random.nextFloat()` drawn for each need in
/// turn until one is met (water breathing under water, fire resistance while burning or
/// just burnt, healing when hurt, swiftness for a far target).
fn choose(tick: &mut MobTick<'_>) -> Option<Potion> {
    let effects = &tick.mob.body.effects;
    let (breathes, resists, swift) = (
        effects.has(Effect::WATER_BREATHING),
        effects.has(Effect::FIRE_RESISTANCE),
        effects.has(Effect::SPEED),
    );
    if tick.draws.next_float() < 0.15 && tick.living.eye_in_water(tick.world) && !breathes {
        return Some(Potion::WaterBreathing);
    }
    if tick.draws.next_float() < 0.15 && (tick.living.is_on_fire() || burnt(tick)) && !resists {
        return Some(Potion::FireResistance);
    }
    let max = tick
        .mob
        .body
        .attributes
        .value(tick.kind, attribute(ids::MAX_HEALTH)) as f32;
    if tick.draws.next_float() < 0.05 && tick.mob.body.vitals.health < max {
        return Some(Potion::Healing);
    }
    // `getTarget() != null && !hasEffect(SPEED) && distanceToSqr(target) > 121`.
    let target_far = target_position(tick)
        .is_some_and(|at| crate::physics::distance_sqr(tick.living.body.position, at) > 121.0);
    if tick.draws.next_float() < 0.5 && target_far && !swift {
        return Some(Potion::Swiftness);
    }
    None
}

/// Where the witch's target stands (`getTarget()`, held while it is a valid one), where
/// the view has it.
fn target_position(tick: &MobTick<'_>) -> Option<crate::physics::Vec3> {
    let key = tick.mob.body.combat.target?;
    let looked = match tick.world.entity(key) {
        Some(seen) => Looked::of_seen(&seen),
        None => tick
            .world
            .players()
            .iter()
            .find(|player| player.key == key)
            .map(Looked::of_player)?,
    };
    sensing::valid_target(&looked).then_some(looked.position)
}

/// The raiders (`Raider`), whom a witch heals rather than harms.
const RAIDERS: [i32; 6] = [
    entity_types::PILLAGER,
    entity_types::VINDICATOR,
    entity_types::EVOKER,
    entity_types::WITCH,
    entity_types::ILLUSIONER,
    entity_types::RAVAGER,
];

/// What a witch reads of its target to choose a potion: its motion
/// (`getDeltaMovement`), its health and the effects it has; a player's as of full health,
/// with none and still.
struct Aimed {
    motion: crate::physics::Vec3,
    health: f32,
    slowed: bool,
    poisoned: bool,
    weakened: bool,
}

fn aimed(ai: &Ai<'_, '_>, target: &Looked) -> Aimed {
    let record = ai.tick.world.entity(target.key);
    let mob = record
        .as_ref()
        .and_then(|seen| Mob::decode(&seen.entity.data).ok());
    let has = |name: &str| {
        Effect::named(name)
            .is_some_and(|effect| mob.as_ref().is_some_and(|mob| mob.body.effects.has(effect)))
    };
    Aimed {
        motion: record.map_or([0.0; 3], |seen| seen.entity.motion),
        health: mob.as_ref().map_or(20.0, |mob| mob.body.vitals.health),
        slowed: has("minecraft:slowness"),
        poisoned: has("minecraft:poison"),
        weakened: has("minecraft:weakness"),
    }
}

/// `Witch.performRangedAttack(target, power)` (n7 Witch 493–655): nothing while it
/// drinks; else a splash potion chosen by the target (a raider healed, or given
/// regeneration, and no longer its target; else slowness from 8 blocks on, poison to one
/// of 8 health or more, weakness within 3 one time in four, the float drawn only there;
/// else harming), thrown from a tenth below its eyes at where the target will be,
/// 1.1 below its eyes and lifted by a fifth of the way across, at 0.45 within 2 blocks
/// and 0.75 beyond, with an uncertainty of 8 (`region/thrown.rs` makes it); then
/// `witch_throw` at a pitch of its own draw.
pub(crate) fn perform_ranged_attack(ai: &mut Ai<'_, '_>, target: &Looked, _power: f32) {
    if illagers::own_of::<WitchState>(&ai.tick.mob)
        .drinking
        .is_some()
    {
        return;
    }
    let read = aimed(ai, target);
    let [x, y, z] = ai.position();
    let dx = target.position[0] + read.motion[0] - x;
    let dy = target.eye_y - 1.100_000_023_841_858 - y;
    let dz = target.position[2] + read.motion[2] - z;
    let across = (dx * dx + dz * dz).sqrt();
    let raider = target
        .kind
        .is_some_and(|kind| RAIDERS.contains(&i32::from(kind)));
    let potion = if raider {
        let potion = if read.health <= 4.0 {
            "minecraft:healing"
        } else {
            "minecraft:regeneration"
        };
        sensing::set_target(ai, None);
        potion
    } else if across >= 8.0 && !read.slowed {
        "minecraft:slowness"
    } else if read.health >= 8.0 && !read.poisoned {
        "minecraft:poison"
    } else if across <= 3.0 && !read.weakened && ai.tick.draws.next_float() < 0.25 {
        "minecraft:weakness"
    } else {
        "minecraft:harming"
    };
    let mut stack = ItemStack::new(items::SPLASH_POTION, 1);
    stack
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some(potion.to_owned()),
            ..PotionContents::default()
        }));
    let eyes = ai.eye_y();
    ai.tick.out.local.push(Local::Shoot(Shot {
        kind: u16::try_from(entity_types::SPLASH_POTION).expect("entity type ids fit 16 bits"),
        stack,
        weapon: None,
        at: [x, eyes - crate::physics::thrown::BELOW_THE_EYES, z],
        direction: [dx, dy + across * 0.2, dz],
        power: if across <= 2.0 { 0.45 } else { 0.75 },
        uncertainty: 8.0,
        from_mob: None,
    }));
    // No mob is silent.
    let pitch = 0.8 + ai.tick.draws.next_float() * 0.4;
    ai.tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_WITCH_THROW,
        volume: 1.0,
        pitch,
    });
}

/// `getLastDamageSource()` is one of `#is_fire`.
fn burnt(tick: &MobTick<'_>) -> bool {
    tick.mob
        .body
        .last_damage
        .source(tick.world.game_time())
        .and_then(|(id, _)| DamageType::of_id(id))
        .is_some_and(|kind| kind.is("minecraft:is_fire"))
}

/// The start of a drink: the potion in its main hand, its use's ticks, the synced flag,
/// `witch_drink` at a pitch of its own draw, and the drinking's slowness on.
fn start_drinking(tick: &mut MobTick<'_>, own: &mut WitchState, potion: Potion) {
    tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
        Some(ItemStack::new(items::POTION, 1));
    own.using_time = POTION_USE_TICKS;
    own.drinking = Some(potion);
    // No mob is silent.
    let pitch = 0.8 + tick.draws.next_float() * 0.4;
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_WITCH_DRINK,
        volume: 1.0,
        pitch,
    });
    let kind = tick.kind;
    if let Some(speed) = tick
        .mob
        .body
        .attributes
        .instance(kind, attribute(ids::MOVEMENT_SPEED))
    {
        speed.remove_modifier(DRINKING);
        speed.add_modifier(Modifier {
            id: DRINKING.to_owned(),
            amount: -0.25,
            operation: ADD_VALUE,
        });
    }
}

impl MobType for Witch {
    /// `Witch.registerGoals`, after `Raider`'s.
    fn register_goals(&self, setup: &mut Setup) {
        illagers::register_raider_goals(setup);
        setup.add_goal(1, FloatGoal);
        setup.add_goal(
            2,
            RangedAttackGoal {
                speed: 1.0,
                interval_min: 60,
                interval_max: 60,
                radius: 10.0,
                perform: perform_ranged_attack,
            },
        );
        setup.add_goal(2, RandomStrollGoal::avoiding_water(1.0));
        setup.add_goal(3, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(3, RandomLookAroundGoal);
        crate::mobs::ai::targets::register(setup, crate::mobs::ai::targets::WITCH);
    }

    /// `Raider.finalizeSpawn`: the leader's draw, which a witch never passes for.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        illagers::finalize_spawn(clustine_data::entity_types::WITCH as u16, spawn, false);
    }

    /// `Witch.aiStep` before `Raider`'s, on a living witch: a drink under way counted
    /// down and, at its end, taken; otherwise the choice of a potion; then the particles'
    /// draw.
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        if tick.mob.body.vitals.dying() {
            return;
        }
        let mut own = illagers::own_of::<WitchState>(&tick.mob);
        if let Some(potion) = own.drinking {
            let left = own.using_time;
            own.using_time -= 1;
            if left <= 0 {
                own.drinking = None;
                own.using_time = 0;
                let now = crate::mobs::body::moment(tick);
                finish_drinking(tick, potion, now);
            }
        } else if let Some(potion) = choose(tick) {
            start_drinking(tick, &mut own, potion);
        }
        if tick.draws.next_float() < 7.5e-4 {
            tick.out.events.push(Event::Entity(PARTICLES_EVENT));
        }
        illagers::update_own(&mut tick.mob, |state: &mut WitchState| *state = own);
    }

    /// `Witch.getDamageAfterMagicAbsorb`: what `#witch_resistant_to` hurts it is cut to
    /// 0.15 of itself. Named: what its own potions do to it (`getEntity() == this`), which
    /// the game makes nothing, is cut as other magic, the hook knowing no key of the
    /// witch's.
    fn magic_absorbed(&self, _: &Mob, source: &DamageSource, amount: f32) -> f32 {
        if source.kind.is("minecraft:witch_resistant_to") {
            amount * 0.15
        } else {
            amount
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        -super::animal::light_cost(ai, pos)
    }

    fn remove_when_far_away(&self, _: u16, mob: &Mob, _: u32, distance: f64) -> bool {
        illagers::remove_when_far_away(mob, distance)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        illagers::experience(5, mob, draws)
    }

    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        illagers::from_template(mob, tag);
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_WITCH_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_WITCH_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_WITCH_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let drinking = illagers::own_of::<WitchState>(mob).drinking.is_some();
        let mut values = illagers::shown(mob);
        values.push((USING_ITEM_INDEX, DataShown::Boolean(drinking), !drinking));
        values
    }
}
