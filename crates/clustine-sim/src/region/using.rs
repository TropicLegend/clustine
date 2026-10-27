//! A player's use of an item over time, and their items' cooldowns
//! (`docs/groundwork/n9-plan.md`, section 5.5; `LivingEntity.startUsingItem`,
//! `updatingUsingItem`, `releaseUsingItem`, `completeUsingItem` and `stopUsingItem`;
//! `ItemCooldowns`).
//!
//! A use begins where the item's `use` starts one (a bow drawn, a crossbow charged, a
//! trident raised, a meal or a drink begun, a shield raised, a spyglass held to the
//! eye), ticks where the game's `Player.tick` ticks it (after the players' look at their
//! main hand: `region.rs`), and ends at its last tick (`finishUsingItem`), when the
//! player lets go (`RELEASE_USE_ITEM`), when the hand holds another item, or when the
//! main hand's slot changes. Its beginning and end are heard (`item_interact_start` and
//! `item_interact_finish`) and shown to the others by the living flags; the player's own
//! client foresees them. What each item does in its use is its part's: the bow's, the
//! crossbow's and the trident's `throws.rs` (S4), eating and drinking `eating.rs` (S1).
//!
//! A use goes on across a hand-over, which the client does not see; a leave ends it, as
//! the game's. Cooldowns are kept on the region's ticks, and a hand-over carries what
//! each has run, as it carries the attack's reset.
//!
//! Named differences: the living flags after a use of the off hand are 0, where the
//! game's `stopUsingItem` takes down only the flag of using and leaves that of the off
//! hand, which shows nothing while the other is down; the equipment the game sends at a
//! use's start
//! (`updatePlayersWithNewEquipment`) is not sent again, since the others are shown the
//! hand already; a goat horn's and a brush's uses wait for their items' rules.

use clustine_data::{ITEMS, ItemComponents, items, sounds};
use clustine_world::{ItemStack, PlayerId};

use super::Region;
use super::vibrations::{Context, Source, Vibration};
use crate::api::{
    Cooldown, Hand, PlayerEvent, RegionEvent, SOUND_SOURCE_PLAYERS, TickOutput, Using,
};
use crate::shown::DataShown;

/// The living entity's flags (`DATA_LIVING_ENTITY_FLAGS`), data index 8: using, and with
/// the off hand.
const LIVING_FLAGS: u8 = 8;
const USING: u8 = 1;
const OFF_HAND: u8 = 2;

/// `Item.getUseDuration` of the bow, the trident and the shield: an hour.
const HELD_FOR_AN_HOUR: i32 = 72_000;

/// `SpyglassItem.USE_DURATION`.
const SPYGLASS_DURATION: i32 = 1200;

/// `ItemStack.getUseDuration` of `stack`, where its `use` starts a use: `None` for an
/// item whose use does not.
pub(crate) fn use_duration(stack: &ItemStack) -> Option<i32> {
    match stack.item {
        items::BOW | items::TRIDENT | items::SHIELD => Some(HELD_FOR_AN_HOUR),
        items::SPYGLASS => Some(SPYGLASS_DURATION),
        // `CrossbowItem.getUseDuration`: its charge and three ticks more.
        items::CROSSBOW => Some(crate::items::crossbow::charge_duration(stack) + 3),
        item => ItemComponents::of(item)
            .consumable
            // `Consumable.consumeTicks`: `(int) (consumeSeconds · 20)`.
            .map(|consumable| (consumable.consume_seconds * 20.0) as i32),
    }
}

/// `Item.useOnRelease`: the crossbow's use ends on release, not at its last tick.
fn uses_on_release(item: i32) -> bool {
    item == items::CROSSBOW
}

/// `ItemCooldowns.getCooldownGroup`: the stack's `use_cooldown` group, else the item.
pub(crate) fn cooldown_group(stack: &ItemStack) -> String {
    use clustine_world::components::{Component, Kind};
    if let Some(Component::UseCooldown(cooldown)) = stack.components.get(Kind::UseCooldown)
        && let Some(group) = &cooldown.group
    {
        return group.clone();
    }
    if !stack.components.removes(Kind::UseCooldown)
        && let Some(group) = ItemComponents::of(stack.item)
            .use_cooldown
            .and_then(|cooldown| cooldown.group)
    {
        return group.to_owned();
    }
    usize::try_from(stack.item)
        .ok()
        .and_then(|item| ITEMS.get(item))
        .map_or_else(String::new, |item| item.name.to_owned())
}

impl Region {
    /// The stack in `id`'s hand `hand`.
    pub(crate) fn in_hand(&self, id: PlayerId, hand: Hand) -> Option<ItemStack> {
        let player = self.players.get(&id)?;
        match hand {
            Hand::Main => player.held(),
            Hand::Off => player.inventory.0[clustine_world::OFF_HAND].clone(),
        }
    }

    /// The use of the player `id`'s item in `hand` where its `use` starts one
    /// (`startUsingItem`): whether it did, so that the item's other uses are not tried.
    /// An item on cooldown is used not at all (`Item.use`'s `PASS`), and a charged
    /// crossbow shoots at once (`throws.rs`).
    pub(crate) fn start_item_use(
        &mut self,
        id: PlayerId,
        hand: Hand,
        output: &mut TickOutput,
    ) -> bool {
        let Some(stack) = self.in_hand(id, hand) else {
            return false;
        };
        if self.is_on_cooldown(id, &stack) {
            return true;
        }
        let Some(duration) = use_duration(&stack) else {
            return false;
        };
        if stack.item == items::CROSSBOW && crate::items::crossbow::is_charged(&stack) {
            self.shoot_charged_crossbow(id, hand, output);
            return true;
        }
        let Some(player) = self.players.get_mut(&id) else {
            return false;
        };
        if player.using.is_some() {
            return true;
        }
        player.using = Some(Using {
            hand,
            item: stack.item,
            remaining: duration,
        });
        self.journal.players.insert(id);
        let feet = player.pose.position;
        let entity = player.entity_id;
        self.show_living_flags(id, output);
        self.vibrate(
            Vibration::ITEM_INTERACT_START,
            [feet.x, feet.y, feet.z],
            Context::of(Some(Source::of(entity)), None),
        );
        // `SpyglassItem.use`: its sound, heard by the others.
        if stack.item == items::SPYGLASS {
            output.events.push(RegionEvent::Sound {
                sound: sounds::ITEM_SPYGLASS_USE,
                source: SOUND_SOURCE_PLAYERS,
                position: feet,
                volume: 1.0,
                pitch: 1.0,
                except: Some(id),
            });
        }
        true
    }

    /// `updatingUsingItem` of every player using an item, in the order of the players:
    /// stopped where the hand holds another item, else `updateUsingItem` (the item's
    /// `onUseTick`, the count down, and the end at 0 unless it ends on release).
    pub(crate) fn tick_uses(&mut self, output: &mut TickOutput) {
        let using: Vec<(PlayerId, Using)> = self
            .players
            .iter()
            .filter_map(|(id, player)| Some((*id, player.using?)))
            .collect();
        for (id, using) in using {
            let same = self
                .in_hand(id, using.hand)
                .is_some_and(|stack| stack.item == using.item);
            if !same {
                self.stop_using(id, output);
                continue;
            }
            self.update_using(id, using, output);
        }
    }

    /// `updateUsingItem`.
    fn update_using(&mut self, id: PlayerId, using: Using, output: &mut TickOutput) {
        self.use_tick(id, using, output);
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let Some(now) = player.using.as_mut() else {
            return;
        };
        now.remaining -= 1;
        let remaining = now.remaining;
        self.journal.players.insert(id);
        if remaining == 0 && !uses_on_release(using.item) {
            self.complete_using(id, output);
        }
    }

    /// `ItemStack.onUseTick` of the item in use: the crossbow's charge, the mouthfuls of
    /// a meal.
    fn use_tick(&mut self, id: PlayerId, using: Using, output: &mut TickOutput) {
        match using.item {
            items::CROSSBOW => self.charge_crossbow(id, using, output),
            item if ItemComponents::of(item).consumable.is_some() => {
                self.consume_tick(id, using, output);
            }
            _ => {}
        }
    }

    /// `releaseUsingItem` of the player `id` (`RELEASE_USE_ITEM`): the item's
    /// `releaseUsing` where the hand holds the item the use was begun with (a bow loosed,
    /// a trident thrown), then the crossbow's last tick, then the end of the use.
    pub(crate) fn release_use(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(using) = self.players.get(&id).and_then(|player| player.using) else {
            return;
        };
        let same = self
            .in_hand(id, using.hand)
            .is_some_and(|stack| stack.item == using.item);
        if same {
            self.release_using(id, using, output);
            if uses_on_release(using.item)
                && self.players.get(&id).is_some_and(|p| p.using.is_some())
            {
                self.update_using(id, using, output);
            }
        }
        self.stop_using(id, output);
    }

    /// `completeUsingItem` at a use's last tick: `finishUsingItem` (a meal eaten), then
    /// the end of the use.
    fn complete_using(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(using) = self.players.get(&id).and_then(|player| player.using) else {
            return;
        };
        if ItemComponents::of(using.item).consumable.is_some() {
            self.finish_consuming(id, using, output);
        }
        self.stop_using(id, output);
    }

    /// `stopUsingItem`: the flags down, `item_interact_finish` heard, the use gone.
    pub(crate) fn stop_using(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if player.using.take().is_none() {
            return;
        }
        self.journal.players.insert(id);
        let feet = player.pose.position;
        let entity = player.entity_id;
        self.show_living_flags(id, output);
        self.vibrate(
            Vibration::ITEM_INTERACT_FINISH,
            [feet.x, feet.y, feet.z],
            Context::of(Some(Source::of(entity)), None),
        );
    }

    /// The living flags of the player `id`'s entity as their use makes them, shown to
    /// the others.
    fn show_living_flags(&self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let flags = player.using.map_or(0, |using| {
            USING
                | match using.hand {
                    Hand::Main => 0,
                    Hand::Off => OFF_HAND,
                }
        });
        output.events.push(RegionEvent::PlayerData {
            entity: player.entity_id,
            chunk: player.chunk(),
            values: vec![(LIVING_FLAGS, DataShown::Byte(flags))],
        });
    }

    /// Whether the group of `stack` is on cooldown for the player `id`
    /// (`ItemCooldowns.isOnCooldown`).
    pub(crate) fn is_on_cooldown(&self, id: PlayerId, stack: &ItemStack) -> bool {
        let group = cooldown_group(stack);
        self.players.get(&id).is_some_and(|player| {
            player
                .cooldowns
                .iter()
                .any(|cooldown| cooldown.group == group && cooldown.end > self.tick)
        })
    }

    /// `ItemCooldowns.addCooldown` of the group of `stack` for `ticks`, which the
    /// player's client is told (`ServerItemCooldowns.onCooldownStarted`).
    pub(crate) fn add_cooldown(
        &mut self,
        id: PlayerId,
        stack: &ItemStack,
        ticks: i32,
        output: &mut TickOutput,
    ) {
        let group = cooldown_group(stack);
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.cooldowns.retain(|cooldown| cooldown.group != group);
        player.cooldowns.push(Cooldown {
            group: group.clone(),
            start: tick,
            end: tick + u64::try_from(ticks.max(0)).unwrap_or(0),
        });
        self.journal.players.insert(id);
        output
            .player_events
            .push((id, PlayerEvent::Cooldown { group, ticks }));
    }

    /// `ItemCooldowns.tick` of every player: the cooldowns whose end has come go, and
    /// each player is told (`onCooldownEnded`'s packet of 0).
    pub(crate) fn tick_cooldowns(&mut self, output: &mut TickOutput) {
        let tick = self.tick;
        for (id, player) in &mut self.players {
            if !player.cooldowns.iter().any(|cooldown| cooldown.end <= tick) {
                continue;
            }
            player.cooldowns.retain(|cooldown| {
                let ended = cooldown.end <= tick;
                if ended {
                    output.player_events.push((
                        *id,
                        PlayerEvent::Cooldown {
                            group: cooldown.group.clone(),
                            ticks: 0,
                        },
                    ));
                }
                !ended
            });
            self.journal.players.insert(*id);
        }
    }
}
