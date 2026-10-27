//! A mob's use of an item over time (`docs/groundwork/n9-plan.md`, section 5.5, "On a
//! mob"; `LivingEntity.startUsingItem`, `updatingUsingItem`, `updateUsingItem`,
//! `releaseUsingItem`, `completeUsingItem` and `stopUsingItem`): the hand it is made
//! with, the item it was begun with, the ticks it has left, and the living flags it
//! leaves (`DATA_LIVING_ENTITY_FLAGS`, data index 8), which clients draw the use by.
//!
//! The use is updated in the mob's tick after its base tick and before its `aiStep`,
//! where `LivingEntity.tick` has `updatingUsingItem`, so a goal that waits on the use
//! (`UseItemGoal.canContinueToUse`) sees it as this tick left it. The stack is read
//! from the hand, not copied (finding 19): the game's `useItem` is the hand's stack
//! itself, taken again from the hand at every update, and compared with it by its item
//! alone (`isSameItem`).
//!
//! What an item does while it is used and at the end is the item's: of the mobs' uses
//! only drinking is here, the wandering trader's potion and milk (`Consumable`'s
//! mouthfuls and `onConsume`, with the type's own sound where it has one,
//! `OverrideConsumeSound`). The witch does not use this part: its drinking is its own,
//! as the game's is, and the living flags are never sent for it.
//!
//! Named differences: an item's `use_effects` component, which can silence the use's
//! game events, is not in the data, so both are always heard (as a player's are,
//! `region/using.rs`); a meal's crumbs (`spawnItemParticles`), the potions' registry
//! (only invisibility is known), and `on_consume_effects` other than milk's clearing
//! wait for the mobs that eat (S1); an item whose use is not a meal or a drink lasts no
//! ticks, as no mob uses one through this part yet.

use clustine_data::{ConsumeEffect, ItemComponents, SOUND_EVENTS, UseAnimation};
use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind};
use serde::{Deserialize, Serialize};

use super::effects::{self, Effect};
use super::{Event, moment};
use crate::api::Hand;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::part::{Part, body_ids};
use crate::mobs::tick::{Local, MobTick};

/// The living flags' bits: an item in use, and with the off hand.
pub(crate) const USING: u8 = 1;
pub(crate) const OFF_HAND: u8 = 2;

/// `Potions.INVISIBILITY`'s effect: invisibility for 3600 ticks (Potions.txt 164–199).
const INVISIBILITY_TICKS: i32 = 3600;

/// A use under way: the hand, the item it was begun with, and the ticks left
/// (`useItemRemaining`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InUse {
    pub(crate) hand: Hand,
    pub(crate) item: i32,
    pub(crate) remaining: i32,
}

/// The use part: the use under way, if any, and the living flags as the game's synced
/// byte has them. These are not only the use's: `stopUsingItem` takes down the using
/// bit alone, so an off hand's use leaves its bit (2) up until the next use begins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ItemUse {
    pub(crate) using: Option<InUse>,
    pub(crate) flags: u8,
}

impl Part for ItemUse {
    const ID: u16 = body_ids::USING;
    const VERSION: u8 = 1;
}

/// `isUsingItem()`.
pub(crate) fn is_using(mob: &Mob) -> bool {
    mob.body.using.using.is_some()
}

fn slot(hand: Hand) -> usize {
    usize::from(match hand {
        Hand::Main => slots::MAIN_HAND,
        Hand::Off => slots::OFF_HAND,
    })
}

/// `getItemInHand(hand)`: none for an empty slot.
fn in_hand(mob: &Mob, hand: Hand) -> Option<&ItemStack> {
    mob.body.equipment.slots[slot(hand)]
        .as_ref()
        .filter(|stack| stack.count > 0)
}

/// `ItemStack.getUseDuration(entity)` of the items a mob uses: a meal's or a drink's
/// `consumeTicks`, `(int) (consumeSeconds · 20)`; 0 for any other, whose use would never
/// end of itself.
fn use_duration(stack: &ItemStack) -> i32 {
    ItemComponents::of(stack.item)
        .consumable
        .map_or(0, |consumable| (consumable.consume_seconds * 20.0) as i32)
}

/// `startUsingItem(hand)`: nothing with an empty hand or a use under way; otherwise the
/// use of the hand's item for its duration, the flags up (using, and with the off hand
/// or not) and `item_interact_start` heard. The equipment the game sends again
/// (`updatePlayersWithNewEquipment`) is the hand's, which clients are shown already.
pub(crate) fn start_using(tick: &mut MobTick<'_>, hand: Hand) {
    let Some(stack) = in_hand(&tick.mob, hand) else {
        return;
    };
    if is_using(&tick.mob) {
        return;
    }
    let using = InUse {
        hand,
        item: stack.item,
        remaining: use_duration(stack),
    };
    let part = &mut tick.mob.body.using;
    part.using = Some(using);
    part.flags |= USING;
    match hand {
        Hand::Main => part.flags &= !OFF_HAND,
        Hand::Off => part.flags |= OFF_HAND,
    }
    game_event(tick, "minecraft:item_interact_start");
}

/// `updatingUsingItem()`, in the mob's tick before its `aiStep`: with the item the use
/// was begun with still in its hand, `updateUsingItem` (the item's `onUseTick`, a tick
/// less, and at none the end, `completeUsingItem`); with anything else there, the use
/// stopped.
pub(crate) fn updating(tick: &mut MobTick<'_>) {
    let Some(using) = tick.mob.body.using.using else {
        return;
    };
    let same = in_hand(&tick.mob, using.hand).is_some_and(|stack| stack.item == using.item);
    if !same {
        stop_using(tick);
        return;
    }
    update(tick, using);
}

/// `updateUsingItem(stack)`. No item of a mob's use ends on release
/// (`useOnRelease`, the crossbow's), so each ends at its last tick.
fn update(tick: &mut MobTick<'_>, using: InUse) {
    on_use_tick(tick, using);
    let Some(now) = tick.mob.body.using.using.as_mut() else {
        return;
    };
    now.remaining -= 1;
    if now.remaining == 0 {
        complete_using(tick);
    }
}

/// `releaseUsingItem()`: the item's `releaseUsing` where the hand holds the item the
/// use was begun with (none of a mob's items does anything on release yet), then the end
/// of the use.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "no mob lets go of a use yet: a bow's or a crossbow's (N9 S4) will"
    )
)]
pub(crate) fn release_using(tick: &mut MobTick<'_>) {
    stop_using(tick);
}

/// `completeUsingItem()` at the use's last tick: `finishUsingItem` (a drink drunk), what
/// it leaves put in the hand, then the end of the use. The game first checks that the
/// hand still holds the stack it updated (`releaseUsingItem` otherwise), which nothing
/// between the two can change here.
fn complete_using(tick: &mut MobTick<'_>) {
    let Some(using) = tick.mob.body.using.using else {
        return;
    };
    if let Some(stack) = in_hand(&tick.mob, using.hand).cloned() {
        let left = finish_using_item(tick, stack);
        tick.mob.body.equipment.slots[slot(using.hand)] = left;
    }
    stop_using(tick);
}

/// `stopUsingItem()`: the using bit down (the off hand's stays), and where a use was
/// under way `item_interact_finish` heard.
pub(crate) fn stop_using(tick: &mut MobTick<'_>) {
    let part = &mut tick.mob.body.using;
    let was = part.using.take().is_some();
    part.flags &= !USING;
    if was {
        game_event(tick, "minecraft:item_interact_finish");
    }
}

/// `gameEvent(event)` at the mob (`ItemStack.causeUseVibration`).
fn game_event(tick: &mut MobTick<'_>, name: &'static str) {
    tick.out.local.push(Local::GameEventAt {
        name,
        at: tick.living.body.position,
        affected: None,
    });
}

/// `ItemStack.onUseTick` of the item in use: a meal's or a drink's mouthful where
/// `shouldEmitParticlesAndSounds(remaining)` holds, from the eighth tick of a drink on
/// (`(int) (consumeTicks · 0.21875)` ticks used) every fourth.
fn on_use_tick(tick: &mut MobTick<'_>, using: InUse) {
    let Some(consumable) = ItemComponents::of(using.item).consumable else {
        return;
    };
    let Some(stack) = in_hand(&tick.mob, using.hand).cloned() else {
        return;
    };
    let ticks = (consumable.consume_seconds * 20.0) as i32;
    let used = ticks - using.remaining;
    let start = (ticks as f32 * 0.21875) as i32;
    if used > start && using.remaining % 4 == 0 {
        mouthful(tick, &stack);
    }
}

/// `Consumable.emitParticlesAndSounds(random, mob, stack, count)`: four draws of the
/// mob's own stream whatever the item (`nextBoolean`, `triangle(1, 0.2)`'s two floats,
/// `randomBetween(0.9, 1)`'s one), then the sound: a drink's at half volume and the last
/// draw's pitch, a meal's at the first draw's volume and the triangle's pitch. The type's
/// own sound where it has one (`OverrideConsumeSound`), else the item's. A meal's crumbs
/// (5 a mouthful, 16 at the end) are not thrown: no mob eats through this part yet.
fn mouthful(tick: &mut MobTick<'_>, stack: &ItemStack) {
    let Some(consumable) = ItemComponents::of(stack.item).consumable else {
        return;
    };
    let volume = if tick.draws.next_bool() { 0.5 } else { 1.0 };
    let triangle = 1.0 + 0.2 * (tick.draws.next_float() - tick.draws.next_float());
    let between = tick.draws.next_float() * (1.0 - 0.9) + 0.9;
    let drink = consumable.animation == UseAnimation::Drink;
    let (volume, pitch) = if drink {
        (0.5, between)
    } else {
        (volume, triangle)
    };
    let sound = tick
        .mob_type
        .consume_sound(stack)
        .or_else(|| sound_named(consumable.sound));
    if let Some(sound) = sound {
        tick.out.events.push(Event::Sound {
            sound,
            volume,
            pitch,
        });
    }
}

/// The sound event of the registry's name `name`, by its id.
fn sound_named(name: &str) -> Option<i32> {
    SOUND_EVENTS
        .iter()
        .position(|sound| *sound == name)
        .and_then(|id| i32::try_from(id).ok())
}

/// `ItemStack.finishUsingItem(level, mob)` of `stack`, the hand's: a meal's or a drink's
/// `Consumable.onConsume` (the last mouthful, the stack's listeners, its
/// `on_consume_effects`, `drink` or `eat` heard, one of it used up), then
/// `applyAfterUseComponentSideEffects`: where it is used up, its `use_remainder`. What
/// is left in the hand.
fn finish_using_item(tick: &mut MobTick<'_>, mut stack: ItemStack) -> Option<ItemStack> {
    let components = ItemComponents::of(stack.item);
    let Some(consumable) = components.consumable else {
        return Some(stack);
    };
    mouthful(tick, &stack);
    let kind = tick.kind;
    let now = moment(tick);
    // The listeners: a potion's contents, of which only invisibility is known.
    if let Some(Component::PotionContents(contents)) = stack.components.get(Kind::PotionContents)
        && contents.potion.as_deref() == Some("minecraft:invisibility")
    {
        effects::add(
            &mut tick.mob,
            kind,
            Effect::INVISIBILITY,
            0,
            INVISIBILITY_TICKS,
            now,
        );
    }
    for effect in consumable.on_consume_effects {
        if let ConsumeEffect::ClearAllEffects = effect {
            effects::remove_all(&mut tick.mob, kind);
        }
    }
    let event = if consumable.animation == UseAnimation::Drink {
        "minecraft:drink"
    } else {
        "minecraft:eat"
    };
    game_event(tick, event);
    stack.count -= 1;
    if stack.count > 0 {
        return Some(stack);
    }
    components
        .use_remainder
        .map(|remainder| ItemStack::new(remainder.item, remainder.count))
}

#[cfg(test)]
#[path = "using_tests.rs"]
mod tests;
