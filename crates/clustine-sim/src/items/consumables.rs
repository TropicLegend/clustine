//! Eating and drinking (`Consumable`, `Consumables`, `ConsumeEffect` and its kinds,
//! `FoodProperties`; `docs/groundwork/n9-plan.md`, section 5.5 and decision 5 of section
//! 18; part S1's): the `consumable` and `food` components of the items' defaults, the
//! draws of a mouthful and of a meal's end, what a meal or a drink gives beside its
//! `on_consume_effects` (the stew's and the ominous bottle's effects), the cooldown it
//! leaves and what is left in the hand (`use_remainder`). The region's part, which
//! applies them to a player, is `region/eating.rs`.
//!
//! Named differences: the `consumable` and `food` of a stack are its item's, as the set
//! of components Clustine keeps on a stack has neither; an effect set given by a tag in
//! `remove_effects` removes nothing, as Clustine keeps no effect tags and no item's data
//! names one.

use clustine_data::{
    Consumable, ConsumeEffect, EffectSet, Food, ItemComponents, SOUND_EVENTS, UseAnimation,
};
use clustine_world::ItemStack;
use clustine_world::components::{Component, EffectInstance, Kind};

use crate::mobs::body::effects::Effect;
use crate::region::Random;

/// `OminousBottleAmplifier.onConsume`'s bad omen: 120 000 ticks, neither ambient nor
/// visible, with its icon.
const BAD_OMEN_TICKS: i32 = 120_000;

/// The stack's `consumable` (its item's).
pub(crate) fn consumable_of(stack: &ItemStack) -> Option<Consumable> {
    ItemComponents::of(stack.item).consumable
}

/// The stack's `food` (its item's).
pub(crate) fn food_of(stack: &ItemStack) -> Option<Food> {
    ItemComponents::of(stack.item).food
}

/// `Consumable.consumeTicks()`: `(int) (consumeSeconds · 20)`.
pub(crate) fn consume_ticks(consumable: &Consumable) -> i32 {
    (consumable.consume_seconds * 20.0) as i32
}

/// `Consumable.shouldEmitParticlesAndSounds(remaining)`: once more than
/// `(int) (consumeTicks · 0.21875)` ticks of the use have gone, every fourth.
pub(crate) fn should_emit(consumable: &Consumable, remaining: i32) -> bool {
    let ticks = consume_ticks(consumable);
    let used = ticks - remaining;
    let start = (ticks as f32 * 0.218_75) as i32;
    used > start && remaining % 4 == 0
}

/// The sound event of the registry's name `name`, by its id.
pub(crate) fn sound_named(name: &str) -> Option<i32> {
    SOUND_EVENTS
        .iter()
        .position(|sound| *sound == name)
        .and_then(|id| i32::try_from(id).ok())
}

/// A mouthful's sound as `emitParticlesAndSounds` plays it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Mouthful {
    pub(crate) sound: Option<i32>,
    pub(crate) volume: f32,
    pub(crate) pitch: f32,
}

/// `Consumable.emitParticlesAndSounds(random, eater, stack, crumbs)`'s draws from
/// `random`, in the game's order: `nextBoolean` for a meal's volume, `triangle(1, 0.2)`'s
/// two floats for its pitch, `randomBetween(0.9, 1)`'s one for a drink's, then where the
/// item throws crumbs, `spawnItemParticles`' four floats a crumb (on the server
/// `addParticle` shows nothing, but the draws are made). The sound: a drink's at half
/// volume and the last draw's pitch, a meal's at the first draw's volume and the
/// triangle's pitch.
pub(crate) fn mouthful(random: &mut Random, consumable: &Consumable, crumbs: u32) -> Mouthful {
    let volume = if random.next_bool() { 0.5 } else { 1.0 };
    let triangle = 1.0 + 0.2 * (random.next_float() - random.next_float());
    let between = random.next_float() * (1.0 - 0.9) + 0.9;
    if consumable.has_consume_particles {
        for _ in 0..crumbs {
            // The crumb's speed (two floats) and its place (two floats), which the
            // server's level does not show.
            for _ in 0..4 {
                random.next_float();
            }
        }
    }
    let drink = consumable.animation == UseAnimation::Drink;
    let (volume, pitch) = if drink {
        (0.5, between)
    } else {
        (volume, triangle)
    };
    Mouthful {
        sound: sound_named(consumable.sound),
        volume,
        pitch,
    }
}

/// `FoodProperties.onConsume`'s draws from `random`: the meal's sound's pitch
/// (`triangle(1, 0.4)`, two floats), then, for a player, the burp's
/// (`randomBetween(0.9, 1)`, one float).
pub(crate) fn food_pitches(random: &mut Random, player: bool) -> (f32, Option<f32>) {
    let eaten = 1.0 + 0.4 * (random.next_float() - random.next_float());
    let burp = player.then(|| random.next_float() * (1.0 - 0.9) + 0.9);
    (eaten, burp)
}

/// The effects of the stack's `suspicious_stew_effects` (`SuspiciousStewEffects.onConsume`:
/// each `Entry.createEffectInstance()`, of its duration at level 0) and of its
/// `ominous_bottle_amplifier` (`OminousBottleAmplifier.onConsume`: bad omen), in that
/// order. The potion's are the region's to read (`region/eating.rs`).
pub(crate) fn listener_effects(stack: &ItemStack) -> Vec<EffectInstance> {
    let mut given = Vec::new();
    if let Some(Component::SuspiciousStewEffects(entries)) =
        stack.components.get(Kind::SuspiciousStewEffects)
    {
        for (effect, duration) in entries {
            given.push(EffectInstance {
                effect: effect.clone(),
                amplifier: 0,
                duration: *duration,
                ambient: false,
                visible: true,
                show_icon: true,
            });
        }
    }
    let amplifier = match stack.components.get(Kind::OminousBottleAmplifier) {
        Some(Component::OminousBottleAmplifier(amplifier)) => Some(*amplifier),
        _ if stack.components.removes(Kind::OminousBottleAmplifier) => None,
        _ => ItemComponents::of(stack.item).ominous_bottle_amplifier,
    };
    if let Some(amplifier) = amplifier
        && let Some(name) = Effect::BAD_OMEN.name()
    {
        given.push(EffectInstance {
            effect: name.to_owned(),
            amplifier,
            duration: BAD_OMEN_TICKS,
            ambient: false,
            visible: false,
            show_icon: true,
        });
    }
    given
}

/// An `apply_effects` entry's effect as an item carries it.
pub(crate) fn instance_of(instance: &clustine_data::EffectInstance) -> EffectInstance {
    EffectInstance {
        effect: instance.effect.to_owned(),
        amplifier: instance.amplifier,
        duration: instance.duration,
        ambient: instance.ambient,
        visible: instance.visible,
        show_icon: instance.show_icon,
    }
}

/// The effects a `remove_effects` entry names, in its order.
pub(crate) fn effects_of(set: EffectSet) -> Vec<Effect> {
    match set {
        EffectSet::Listed(names) => names
            .iter()
            .filter_map(|name| Effect::named(name))
            .collect(),
        EffectSet::Tag(_) => Vec::new(),
    }
}

/// The `on_consume_effects` of the stack's `consumable`, in the data's order.
pub(crate) fn on_consume_effects(consumable: &Consumable) -> &'static [ConsumeEffect] {
    consumable.on_consume_effects
}

/// `UseCooldown.ticks()` of the stack's `use_cooldown` (its patch's, else its item's):
/// `(int) (seconds · 20)`, which `applyAfterUseComponentSideEffects` starts for a
/// player.
pub(crate) fn cooldown_ticks(stack: &ItemStack) -> Option<i32> {
    if let Some(Component::UseCooldown(cooldown)) = stack.components.get(Kind::UseCooldown) {
        return Some((cooldown.seconds.0 * 20.0) as i32);
    }
    if stack.components.removes(Kind::UseCooldown) {
        return None;
    }
    ItemComponents::of(stack.item)
        .use_cooldown
        .map(|cooldown| (cooldown.seconds * 20.0) as i32)
}

/// What a meal leaves (`ItemStack.consume(1, eater)`, then `UseRemainder.convertIntoRemainder`
/// of the stack as it was, `before`): for an eater with infinite materials (a creative
/// player) the stack untouched and nothing more; else one fewer, and where the item has a
/// remainder (a bowl, a bottle, a bucket), the remainder in the hand where the stack is
/// gone, or the stack in the hand and the remainder given besides. The stack in the hand,
/// and the remainder to give.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "every player of Clustine is creative, whose meals use nothing up"
    )
)]
pub(crate) fn eaten(before: &ItemStack, infinite: bool) -> (Option<ItemStack>, Option<ItemStack>) {
    if infinite {
        return (Some(before.clone()), None);
    }
    let mut left = before.clone();
    left.count -= 1;
    let remainder = ItemComponents::of(before.item)
        .use_remainder
        .map(|remainder| ItemStack::new(remainder.item, remainder.count));
    match remainder {
        None => ((left.count > 0).then_some(left), None),
        Some(remainder) if left.count <= 0 => (Some(remainder), None),
        Some(remainder) => (Some(left), Some(remainder)),
    }
}
