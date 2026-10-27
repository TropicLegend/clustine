//! The witch (`Witch`; `n7k-brief.md` 3.6), part K5: a raider that floats, strolls and
//! looks, and in its `aiStep` drinks a potion where it needs one (water breathing under
//! water, fire resistance while it burns, healing when hurt; swiftness after a far
//! target, which it has none of before N9), with the game's draws in the game's order.
//!
//! Named differences: the potion in its hand is a plain potion, as item stacks carry no
//! contents (`n7k-plan.md`, section 10): the client draws an empty bottle, and the
//! potion it drinks is kept in its type state; the witch's own splash potions, which
//! it would throw at its targets and which its magic resistance spares it, are N9's,
//! as are the cooldown of its healing of other raiders and the switch of its player
//! goal, both its target goals'.

use clustine_data::{attributes as ids, items, sounds};
use clustine_world::ItemStack;

use super::illagers::{self, Potion};
use super::{MobType, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{FloatGoal, LookAtPlayerGoal, RandomLookAroundGoal, RandomStrollGoal};
use crate::mobs::ai::selector::flags;
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::attributes::ADD_VALUE;
use crate::mobs::body::effects::{self, Effect};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::body::{Event, Modifier, attribute};
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::types::tamable::Idle;
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
    // `getTarget() != null && !hasEffect(SPEED) && distanceToSqr(target) > 121`: no
    // target before N9.
    let target_far = false;
    if tick.draws.next_float() < 0.5 && target_far && !swift {
        return Some(Potion::Swiftness);
    }
    None
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
fn start_drinking(tick: &mut MobTick<'_>, own: &mut illagers::RaiderState, potion: Potion) {
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
        // `RangedAttackGoal(1.0, 60, 10)`: a target's (N9).
        setup.add_goal(
            2,
            Idle {
                flags: flags::MOVE | flags::LOOK,
            },
        );
        setup.add_goal(2, RandomStrollGoal::avoiding_water(1.0));
        setup.add_goal(3, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(3, RandomLookAroundGoal);
        // `HurtByTargetGoal(Raider)`, `NearestHealableRaiderTargetGoal`,
        // `NearestAttackableWitchTargetGoal(Player)`.
        illagers::register_targets(setup, &[1, 2, 3]);
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
        let mut own = illagers::state_of(&tick.mob);
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
        illagers::update(&mut tick.mob, |state| *state = own);
    }

    /// `Witch.getDamageAfterMagicAbsorb`: what `#witch_resistant_to` hurts it is cut to
    /// 0.15 of itself. (Its own potions' hurt, nothing, is N9's.)
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
        illagers::from_template(mob, tag, |_| {});
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
        let drinking = illagers::state_of(mob).drinking.is_some();
        let mut values = illagers::shown(mob);
        values.push((USING_ITEM_INDEX, DataShown::Boolean(drinking), !drinking));
        values
    }
}
