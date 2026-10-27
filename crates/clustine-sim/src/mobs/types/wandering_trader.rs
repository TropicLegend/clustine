//! The wandering trader (`WanderingTrader`; `docs/groundwork/n7k-brief.md` 2.7 and
//! `n7k-plan.md` section 10), part K2: an ageable mob of goals that floats, drinks an
//! invisibility potion when the night comes and milk when the day does, runs from
//! zombies and illagers, panics when hurt, strolls avoiding water and looks at players
//! and mobs, and leaves when its despawn delay runs out (an egg's has none).
//!
//! Drinking is its own here: what `LivingEntity` does with an item in use
//! (`startUsingItem`, `updatingUsingItem`, `completeUsingItem`) for the two drinks it
//! has, kept in its state ([`Using`]) and shown to clients by the living flags. The
//! `Consumable` component's sounds and its duration are not in the dumps (**Inferred**
//! from the extracted components and the game's code as known: 32 ticks; from the eighth
//! on, every fourth, the drinking sound at half or full volume and a pitch of
//! `triangle(1, 0.2)`, and once more as it is finished).
//!
//! Named differences: it trades nothing, so its trading goals never start, a click opens
//! nothing (the game always has offers and opens their screen) and the statistic of a
//! talk is not kept; its potion is a plain potion in its hand (items carry no
//! components); a template's `wander_target` is not read (a block position's int array).

use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Animal, Own};
use super::{AgeableGroup, Dimensions, Interaction, MobType, SpawnGroup, Spawning, Used, template};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, FloatGoal, Items, LookAtPlayerGoal, MoveTowardsRestrictionGoal, PanicGoal,
    RandomStrollGoal,
};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup, navigation};
use crate::mobs::body::effects::{self, Effect};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::{self, Event, Removed};
use crate::mobs::tick::{Local, MobTick};
use crate::physics;
use crate::shown::DataShown;

/// The synced data's index of `AbstractVillager.DATA_UNHAPPY_COUNTER`, never set of a
/// trader, and `LivingEntity.DATA_LIVING_ENTITY_FLAGS`, whose first bit is an item in use.
const UNHAPPY_INDEX: u8 = 18;
const LIVING_FLAGS_INDEX: u8 = 8;
const USING_ITEM: u8 = 1;

/// A baby's size: `AgeableMob`'s half of the type's (0.6 by 1.95, its eyes at 1.62).
const BABY: Dimensions = Dimensions {
    width: 0.3,
    height: 0.975,
    eye_height: 0.81,
};

/// `Consumable.consumeTicks()` of a potion and of milk: 1.6 seconds (**Inferred**).
pub(crate) const DRINK_TICKS: i32 = 32;

/// `Potions.INVISIBILITY`'s effect: invisibility for 3600 ticks (Potions.txt 164–199).
pub(crate) const INVISIBILITY_TICKS: i32 = 3600;

/// An item in use (`useItem`, `useItemRemaining`): the drink in its main hand, and the
/// ticks left until it is drunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Using {
    pub(crate) item: ItemStack,
    pub(crate) remaining: i32,
}

/// What a wandering trader keeps beyond an ageable mob: its despawn delay (an egg's 0,
/// never), where it wanders to, and its drink in use.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TraderState {
    pub(crate) despawn_delay: i32,
    pub(crate) wander_target: Option<BlockPos>,
    pub(crate) using: Option<Using>,
}

impl Own for TraderState {}

/// A trader's ageable part and its own state.
pub(crate) fn state_of(mob: &Mob) -> (Animal, TraderState) {
    animal::read::<TraderState>(&mob.state)
}

/// Changes a trader's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut TraderState)) {
    let (shared, mut own) = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `Level.isDarkOutside()`: the sky darkened by four or more.
fn dark_outside(ai: &Ai<'_, '_>) -> bool {
    ai.tick.world.sky_darken() >= 4
}

/// The drink a [`UseItemGoal`] holds out: an invisibility potion, or milk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Drink {
    Invisibility,
    Milk,
}

impl Drink {
    fn stack(self) -> ItemStack {
        let item = match self {
            Drink::Invisibility => items::POTION,
            Drink::Milk => items::MILK_BUCKET,
        };
        ItemStack::new(item, 1)
    }

    /// What is left in the hand once drunk (`minecraft:use_remainder`).
    fn remainder(self) -> ItemStack {
        let item = match self {
            Drink::Invisibility => items::GLASS_BOTTLE,
            Drink::Milk => items::BUCKET,
        };
        ItemStack::new(item, 1)
    }

    fn of(item: i32) -> Option<Self> {
        match item {
            items::POTION => Some(Drink::Invisibility),
            items::MILK_BUCKET => Some(Drink::Milk),
            _ => None,
        }
    }

    /// `getConsumeSound(stack)` (WanderingTrader.txt 407–417).
    fn sound(self) -> i32 {
        match self {
            Drink::Invisibility => sounds::ENTITY_WANDERING_TRADER_DRINK_POTION,
            Drink::Milk => sounds::ENTITY_WANDERING_TRADER_DRINK_MILK,
        }
    }
}

/// `UseItemGoal(trader, drink, finishSound, predicate)` (UseItemGoal.txt): where its
/// predicate holds (lambda #0: dark outside and not invisible, for the potion; lambda #1:
/// bright outside and invisible, for milk), the drink in its main hand and in use, until
/// it is drunk; then its hand emptied and the finishing sound at a pitch of
/// `0.9 + nextFloat() · 0.2` of its own random.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UseItemGoal {
    pub(crate) drink: Drink,
    pub(crate) finish_sound: i32,
}

impl Goal for UseItemGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let invisible = ai.tick.mob.body.effects.invisible();
        match self.drink {
            Drink::Invisibility => dark_outside(ai) && !invisible,
            Drink::Milk => !dark_outside(ai) && invisible,
        }
    }

    fn can_continue(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        state_of(&ai.tick.mob).1.using.is_some()
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let stack = self.drink.stack();
        ai.tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = Some(stack.clone());
        start_using(ai.tick, stack);
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        ai.tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = None;
        let pitch = ai.tick.draws.next_float() * 0.2 + 0.9;
        ai.tick.out.events.push(Event::Sound {
            sound: self.finish_sound,
            volume: 1.0,
            pitch,
        });
    }
}

/// `startUsingItem(MAIN_HAND)`: the stack in use for its whole time, and
/// `ITEM_INTERACT_START` at it (a drink's, **Inferred**: `causeUseVibration` is not in
/// the dumps).
fn start_using(tick: &mut MobTick<'_>, stack: ItemStack) {
    if state_of(&tick.mob).1.using.is_some() {
        return;
    }
    update(&mut tick.mob, |own| {
        own.using = Some(Using {
            item: stack,
            remaining: DRINK_TICKS,
        });
    });
    tick.out.local.push(Local::GameEventAt {
        name: "minecraft:item_interact_start",
        at: tick.living.body.position,
        affected: None,
    });
}

/// `Consumable.emitParticlesAndSounds(random, trader, stack, count)` of a drink, which
/// has no particles: its sound at half or full volume (`nextBoolean`) and a pitch of
/// `triangle(1, 0.2)`, of its own random (**Inferred**).
fn drinking_sound(tick: &mut MobTick<'_>, drink: Drink) {
    let volume = if tick.draws.next_bool() { 0.5 } else { 1.0 };
    let pitch = 1.0 + 0.2 * (tick.draws.next_float() - tick.draws.next_float());
    tick.out.events.push(Event::Sound {
        sound: drink.sound(),
        volume,
        pitch,
    });
}

/// `LivingEntity.updatingUsingItem()`, after `baseTick`: with its drink still in its
/// hand, `onUseTick` (the drinking sound every fourth tick after the first seven,
/// **Inferred**) and a tick less, drunk at none (`completeUsingItem`: the sound, the
/// potion's invisibility or milk's end of every effect, `DRINK` at it, what is left of
/// it in its hand); with anything else in its hand, its use stopped.
fn updating_using_item(tick: &mut MobTick<'_>) {
    let Some(using) = state_of(&tick.mob).1.using else {
        return;
    };
    let hand = &tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)];
    let same = hand
        .as_ref()
        .is_some_and(|held| held.count > 0 && held.item == using.item.item);
    let Some(drink) = Drink::of(using.item.item).filter(|_| same) else {
        update(&mut tick.mob, |own| own.using = None);
        return;
    };
    let used = DRINK_TICKS - using.remaining;
    let start = (DRINK_TICKS as f32 * 0.21875) as i32;
    if used > start && using.remaining % 4 == 0 {
        drinking_sound(tick, drink);
    }
    let remaining = using.remaining - 1;
    if remaining > 0 {
        update(&mut tick.mob, |own| {
            if let Some(using) = own.using.as_mut() {
                using.remaining = remaining;
            }
        });
        return;
    }
    drinking_sound(tick, drink);
    let kind = tick.kind;
    let now = body::moment(tick);
    match drink {
        Drink::Invisibility => {
            effects::add(
                &mut tick.mob,
                kind,
                Effect::INVISIBILITY,
                0,
                INVISIBILITY_TICKS,
                now,
            );
        }
        Drink::Milk => {
            effects::remove_all(&mut tick.mob, kind);
        }
    }
    tick.out.local.push(Local::GameEventAt {
        name: "minecraft:drink",
        at: tick.living.body.position,
        affected: None,
    });
    tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = Some(drink.remainder());
    update(&mut tick.mob, |own| own.using = None);
}

/// A goal of trading that never starts, as no player trades with a trader:
/// `TradeWithPlayerGoal` (its flags jump and move) and `LookAtTradingPlayerGoal` (look).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Trading {
    pub(crate) flags: u8,
}

impl Goal for Trading {
    type State = ();

    fn flags(&self) -> u8 {
        self.flags
    }

    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }
}

/// `WanderingTrader$WanderToPositionGoal(trader, stopDistance, speed)`: with a place to
/// wander to farther than `stop_distance` from its centre, a path there each time the
/// last is done (10 blocks of the way at a time where it is farther than 10); as it
/// stops, the place forgotten. An egg's trader has none.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WanderToPositionGoal {
    pub(crate) stop_distance: f64,
    pub(crate) speed: f64,
}

/// `isTooFarAway(pos, distance)`: not `closerToCenterThan`.
fn too_far(ai: &Ai<'_, '_>, target: BlockPos, distance: f64) -> bool {
    !super::turtle::closer_to_centre_than(target, ai.position(), distance)
}

impl Goal for WanderToPositionGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        state_of(&ai.tick.mob)
            .1
            .wander_target
            .is_some_and(|target| too_far(ai, target, self.stop_distance))
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| own.wander_target = None);
        navigation::stop(ai);
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let Some(target) = state_of(&ai.tick.mob).1.wander_target else {
            return;
        };
        if !navigation::is_done(ai) {
            return;
        }
        let position = ai.position();
        let to = if too_far(ai, target, 10.0) {
            let way = physics::normalize([
                f64::from(target.x) - position[0],
                f64::from(target.y) - position[1],
                f64::from(target.z) - position[2],
            ]);
            physics::add(physics::scale(way, 10.0), position)
        } else {
            [
                f64::from(target.x),
                f64::from(target.y),
                f64::from(target.z),
            ]
        };
        navigation::move_to(ai, to, self.speed);
    }
}

/// The zombies' class (`Zombie`), which it runs from.
const ZOMBIES: [u16; 5] = [
    entity_types::ZOMBIE as u16,
    entity_types::HUSK as u16,
    entity_types::DROWNED as u16,
    entity_types::ZOMBIE_VILLAGER as u16,
    entity_types::ZOMBIFIED_PIGLIN as u16,
];

/// `AvoidEntityGoal(trader, class, distance, 0.5, 0.5)` of one class.
fn avoid(class: &'static [u16], distance: f32) -> AvoidEntityGoal {
    AvoidEntityGoal {
        class: Class::Kinds(class),
        max_distance: distance,
        walk_speed: 0.5,
        sprint_speed: 0.5,
    }
}

/// The wandering trader.
pub(crate) struct WanderingTrader;

impl MobType for WanderingTrader {
    /// `AbstractVillager`'s constructor (fire beside a path costs 16, fire is never
    /// walked) and `WanderingTrader.registerGoals` (20–219).
    fn register_goals(&self, setup: &mut Setup) {
        setup.set_malus(clustine_data::PathType::FireInNeighbor, 16.0);
        setup.set_malus(clustine_data::PathType::Fire, -1.0);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(
            0,
            UseItemGoal {
                drink: Drink::Invisibility,
                finish_sound: sounds::ENTITY_WANDERING_TRADER_DISAPPEARED,
            },
        );
        setup.add_goal(
            0,
            UseItemGoal {
                drink: Drink::Milk,
                finish_sound: sounds::ENTITY_WANDERING_TRADER_REAPPEARED,
            },
        );
        setup.add_goal(
            1,
            Trading {
                flags: flags::JUMP | flags::MOVE,
            },
        );
        setup.add_goal(1, avoid(&ZOMBIES, 8.0));
        setup.add_goal(1, avoid(&[entity_types::EVOKER as u16], 12.0));
        setup.add_goal(1, avoid(&[entity_types::VINDICATOR as u16], 8.0));
        setup.add_goal(1, avoid(&[entity_types::VEX as u16], 8.0));
        setup.add_goal(1, avoid(&[entity_types::PILLAGER as u16], 15.0));
        setup.add_goal(1, avoid(&[entity_types::ILLUSIONER as u16], 12.0));
        setup.add_goal(1, avoid(&[entity_types::ZOGLIN as u16], 10.0));
        setup.add_goal(1, PanicGoal { speed: 0.5 });
        setup.add_goal(1, Trading { flags: flags::LOOK });
        setup.add_goal(
            2,
            WanderToPositionGoal {
                stop_distance: 2.0,
                speed: 0.35,
            },
        );
        setup.add_goal(4, MoveTowardsRestrictionGoal { speed: 0.35 });
        setup.add_goal(8, RandomStrollGoal::avoiding_water(0.35));
        setup.add_goal(
            9,
            LookAtPlayerGoal {
                class: Class::Player,
                look_distance: 3.0,
                probability: 1.0,
                only_horizontal: false,
                holds_still: true,
            },
        );
        setup.add_goal(10, LookAtPlayerGoal::new(Class::Mob, 8.0));
    }

    /// `AbstractVillager.finalizeSpawn`: a group of no babies, so no draw; `Mob`'s
    /// follows.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        animal::finalize_ageable(spawn, SpawnGroup::Ageable(AgeableGroup::new(false)));
    }

    /// `updatingUsingItem` in `LivingEntity.tick`, after the entity's.
    fn after_base_tick(&self, tick: &mut MobTick<'_>, _: i32) {
        updating_using_item(tick);
    }

    /// `AgeableMob.aiStep`'s growing, then `maybeDespawn`: a delay above nothing counts
    /// down (no player trades) and at nothing the trader is gone.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        let (_, own) = state_of(&tick.mob);
        if own.despawn_delay > 0 {
            let left = own.despawn_delay - 1;
            update(&mut tick.mob, |own| own.despawn_delay = left);
            if left == 0 {
                tick.out.removed = Some(Removed::Discarded);
            }
        }
    }

    /// `WanderingTrader.mobInteract` (230–281): with anything but a villager's egg, alive,
    /// grown and not trading, it has no offers here, so the use is consumed and nothing
    /// opens; otherwise `AgeableMob`'s (the golden dandelion on a baby).
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let egg = used
            .held
            .as_ref()
            .is_some_and(|stack| stack.count > 0 && stack.item == items::VILLAGER_SPAWN_EGG);
        if !egg && !used.mob.body.vitals.dying() && !animal::is_baby(used.mob) {
            return Interaction::Consume;
        }
        animal::mob_interact(used, Items::Of(&[]), None)
    }

    /// `WanderingTrader.readAdditionalSaveData` (322–349) of a template's tag, with
    /// `AgeableMob`'s: its despawn delay, and its age held at nothing or above.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let (mut shared, mut own) = state_of(mob);
        shared.set_age(template::int(tag, "Age").unwrap_or(0).max(0), 0);
        shared.forced_age = template::int(tag, "ForcedAge").unwrap_or(0);
        shared.age_locked = template::boolean(tag, "AgeLocked").unwrap_or(false);
        own.despawn_delay = template::int(tag, "DespawnDelay").unwrap_or(0);
        mob.state = animal::write(&shared, &own);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    /// `getAmbientSound()`: never trading, so its ambient.
    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_WANDERING_TRADER_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_WANDERING_TRADER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_WANDERING_TRADER_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let using = state_of(mob).1.using.is_some();
        let living = if using { USING_ITEM } else { 0 };
        let mut values = animal::shown(mob);
        values.push((LIVING_FLAGS_INDEX, DataShown::Byte(living), living == 0));
        values.push((UNHAPPY_INDEX, DataShown::Int(0), true));
        values
    }
}
