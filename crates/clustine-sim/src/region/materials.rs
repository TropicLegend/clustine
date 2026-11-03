//! What a player's game mode lets them keep (`docs/groundwork/v-plan.md`, sections 3.1,
//! 4.1 and 4.6, step V1): every use of an item asks the acting player's
//! `hasInfiniteMaterials` (`Abilities.instabuild`, n7 Player.hasInfiniteMaterials @0-7),
//! and a player who has none loses what they place, throw, pour, feed or wear down, as
//! the game's `ItemStack.consume`, `hurtAndBreak` and `ItemUtils.createFilledResult` take
//! it. The client foresees most of it; what the region changes it is told of, slot by
//! slot, as the game's `broadcastChanges` tells it (`PlayerEvent::InventorySlot`).
//!
//! Named differences, each the plan's to lift:
//! - **Across a line** (a click, a use on an entity or a placing done by another region
//!   than the player's, `RemoteStep` and `Effected::OnEntity`), nothing is used up: the
//!   region that does the step has not the player's inventory, and taking the item into
//!   the step and giving back what is left (section 4.6) is V2's. Such a player is
//!   treated as having infinite materials, as everyone was before.
//! - A tool or a weapon that wears out is gone from the hand without the game's sound and
//!   particles of its breaking (`onEquippedItemBroken`'s entity event).
//! - A survival player is **invulnerable until V4**: no hurt reaches them and no mob
//!   takes them as a target (`entity_effects.rs`, `region/mobs.rs`, the region's
//!   `hurt_reaches_players`), though their abilities say otherwise to their client.
//! - A survival player's **break is instant until V2**, with no drops (`updates.rs`), and
//!   their hits break armour stands, vehicles and hanging things at once, as a creative
//!   player's (`EntityUse::Attack`'s `creative`).

use clustine_world::{Abilities, EntityId, ItemStack, OFF_HAND, PlayerId};

use super::Region;
use crate::api::{Hand, PlayerEvent, TickOutput};
use crate::items::enchantments;

/// The game's player attributes `block_interaction_range` and `entity_interaction_range`
/// (n6 Attributes.<clinit> @214 and @350), and the modifiers creative mode adds to them
/// (n7 Player, `CREATIVE_BLOCK_INTERACTION_RANGE_MODIFIER_VALUE` 0.5 and
/// `CREATIVE_ENTITY_INTERACTION_RANGE_MODIFIER_VALUE` 2.0, line 41).
pub(crate) const BLOCK_INTERACTION_RANGE: f64 = 4.5;
pub(crate) const ENTITY_INTERACTION_RANGE: f64 = 3.0;
const CREATIVE_BLOCK_RANGE_MODIFIER: f64 = 0.5;
const CREATIVE_ENTITY_RANGE_MODIFIER: f64 = 2.0;

/// The tolerance the game's handlers add to a reach: 1 for a block
/// (`handleUseItemOn` @84-91, `handlePickItemFromBlock` @19-25,
/// n5 `ServerPlayerGameMode.handleBlockBreakAction` @5-6), 3 for an entity (`handleInteract`
/// @80-89, `handleAttack` @88-99, `handlePickItemFromEntity` @32).
pub(crate) const BLOCK_REACH_TOLERANCE: f64 = 1.0;
pub(crate) const ENTITY_REACH_TOLERANCE: f64 = 3.0;

/// `FoodData.needsFood()` of the player `id`: hunger is V4's
/// (`docs/groundwork/v-plan.md`, section 4.4), and until then every player's food is
/// full, so nobody needs food.
pub(crate) const fn needs_food(_player: PlayerId) -> bool {
    false
}

/// A player's `blockInteractionRange()`: the attribute, with creative's modifier where
/// their mode is creative (the game's modifier is the mode's, not `instabuild`'s).
pub(crate) fn block_interaction_range(mode: clustine_world::GameMode) -> f64 {
    if mode == clustine_world::GameMode::Creative {
        BLOCK_INTERACTION_RANGE + CREATIVE_BLOCK_RANGE_MODIFIER
    } else {
        BLOCK_INTERACTION_RANGE
    }
}

/// A player's `entityInteractionRange()`, as [`block_interaction_range`].
pub(crate) fn entity_interaction_range(mode: clustine_world::GameMode) -> f64 {
    if mode == clustine_world::GameMode::Creative {
        ENTITY_INTERACTION_RANGE + CREATIVE_ENTITY_RANGE_MODIFIER
    } else {
        ENTITY_INTERACTION_RANGE
    }
}

/// Whether a player in the game mode of id `mode`, as a face carries it, is passed over
/// by mobs choosing a target (`EntitySelector.NO_CREATIVE_OR_SPECTATOR` and the like,
/// which read `isCreative`): a creative player, and until V4 every player, a survival
/// player being spared every hurt and every mob's targeting as a creative player is,
/// though their abilities say otherwise to their client (the region's
/// `hurt_reaches_players`, false until V4).
pub(crate) fn untargetable(mode: u8, hurt_reaches_players: bool) -> bool {
    mode == clustine_world::GameMode::Creative.id() || !hurt_reaches_players
}

impl Region {
    /// Lets hurts reach players who are not invulnerable, and mobs take them as targets,
    /// as V4 will (`docs/groundwork/v-plan.md`, section 4.3): for the tests of the mobs'
    /// targeting of survival players, written for when it comes. Nothing else turns it
    /// on before V4.
    pub fn let_hurt_reach_players(&mut self) {
        self.hurt_reaches_players = true;
    }

    /// Whether the region's player `id` is spared a hurt: an invulnerable one (a creative
    /// player's `Abilities.invulnerable`), and until V4 every one (the region's
    /// `hurt_reaches_players`).
    pub(crate) fn spared_hurt(&self, id: PlayerId) -> bool {
        let invulnerable = self
            .abilities_of(id)
            .is_none_or(|abilities| abilities.invulnerable);
        invulnerable || !self.hurt_reaches_players
    }

    /// What the region's player `id`'s game mode lets them do; `None` for a player the
    /// region does not have.
    pub(crate) fn abilities_of(&self, id: PlayerId) -> Option<Abilities> {
        self.players
            .get(&id)
            .map(|player| player.mode.current.abilities())
    }

    /// `Player.hasInfiniteMaterials` of the player `id`: their `instabuild`. A player the
    /// region does not have, whose use another region passed on, is taken to have them
    /// (named in the module: what crosses a line is used up in V2).
    pub(crate) fn has_infinite_materials(&self, id: PlayerId) -> bool {
        self.abilities_of(id)
            .is_none_or(|abilities| abilities.instabuild)
    }

    /// [`Region::has_infinite_materials`] of the player whose entity is `entity`.
    pub(crate) fn entity_has_infinite_materials(&self, entity: EntityId) -> bool {
        self.player_with_entity(entity)
            .is_none_or(|id| self.has_infinite_materials(id))
    }

    /// The hand of `id` that holds `item`: the main hand where both do, as the client
    /// uses the off hand only where the main hand's use did nothing.
    pub(crate) fn hand_with_item(&self, id: PlayerId, item: i32) -> Option<Hand> {
        [Hand::Main, Hand::Off].into_iter().find(|hand| {
            self.in_hand(id, *hand)
                .is_some_and(|stack| stack.item == item)
        })
    }

    /// `ItemStack.consume(count, player)` of what `id` holds in `hand`: shrunk by
    /// `count`, but not for a player with infinite materials, and their client told.
    pub(crate) fn consume_in_hand(
        &mut self,
        id: PlayerId,
        hand: Hand,
        count: i32,
        output: &mut TickOutput,
    ) {
        if self.has_infinite_materials(id) {
            return;
        }
        let Some(stack) = self.in_hand(id, hand) else {
            return;
        };
        let left = (stack.count > count).then(|| stack.with_count(stack.count - count));
        self.set_hand(id, hand, left, output);
    }

    /// [`Region::consume_in_hand`] of the hand of `id` that holds `item` (the used item
    /// of a click or a use, which names the item and not the hand), where one does.
    pub(crate) fn consume_item(
        &mut self,
        id: PlayerId,
        item: i32,
        count: i32,
        output: &mut TickOutput,
    ) {
        if let Some(hand) = self.hand_with_item(id, item) {
            self.consume_in_hand(id, hand, count, output);
        }
    }

    /// [`Region::consume_item`] of the item of the entity `player`'s player, where they
    /// are the region's: a use of a mob or of a hanging thing names the user's entity.
    pub(crate) fn consume_item_of_entity(
        &mut self,
        player: EntityId,
        item: i32,
        count: i32,
        output: &mut TickOutput,
    ) {
        if let Some(id) = self.player_with_entity(player) {
            self.consume_item(id, item, count, output);
        }
    }

    /// `ItemStack.hurtAndBreak(amount, player, hand)` of what `id` holds in `hand`: no
    /// wear for a player with infinite materials (`processDurabilityChange`), else the
    /// amount less what Unbreaking keeps off, drawn from the level's random, and the stack
    /// gone where that wears it out (named in the module: without the break's sound).
    pub(crate) fn hurt_and_break_in_hand(
        &mut self,
        id: PlayerId,
        hand: Hand,
        amount: i32,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let slot = match hand {
            Hand::Main => usize::from(player.selected_slot),
            Hand::Off => OFF_HAND,
        };
        self.hurt_and_break_slot(id, slot, amount, output);
    }

    /// `ItemStack.hurtAndBreak(amount, player, slot)` of the stack in the inventory's
    /// `slot` of `id`, as [`Region::hurt_and_break_in_hand`] wears a hand's.
    pub(crate) fn hurt_and_break_slot(
        &mut self,
        id: PlayerId,
        slot: usize,
        amount: i32,
        output: &mut TickOutput,
    ) {
        if self.has_infinite_materials(id) {
            return;
        }
        let Some(stack) = self
            .players
            .get(&id)
            .and_then(|player| player.inventory.0.get(slot).cloned().flatten())
        else {
            return;
        };
        if clustine_data::ItemComponents::of(stack.item)
            .max_damage
            .is_none()
        {
            return;
        }
        let mut level = self.next_draw();
        let change = enchantments::durability_change(&stack, amount, || level.next_float());
        let worn = enchantments::hurt_and_break(&stack, change);
        if worn.as_ref() != Some(&stack) {
            self.set_inventory_slot(id, slot, worn, output);
        }
    }

    /// `ItemStack.hurtAndConvertOnBreak(amount, into, player, slot)` of what `id` holds
    /// in `hand`: worn as [`Region::hurt_and_break_in_hand`] wears it, and where that
    /// wears it out, `into` in its place with its components and no wear
    /// (`transmuteCopyIgnoreEmpty`, a carrot on a stick worn out to a fishing rod).
    pub(crate) fn hurt_and_convert_on_break(
        &mut self,
        id: PlayerId,
        hand: Hand,
        amount: i32,
        into: i32,
        output: &mut TickOutput,
    ) {
        let Some(before) = self.in_hand(id, hand) else {
            return;
        };
        self.hurt_and_break_in_hand(id, hand, amount, output);
        if self.in_hand(id, hand).is_none() {
            let mut converted = before.with_count(1);
            converted.item = into;
            converted
                .components
                .reset(clustine_world::components::Kind::Damage);
            self.set_hand(id, hand, Some(converted), output);
        }
    }

    /// `updateFallFlying`'s wear of the glider of the region's player `id` (n9
    /// LivingEntity @57-102): one of the slots that glide drawn from their random
    /// (`Util.getRandom`), the elytra on the chest being the only one, and worn by one.
    pub(crate) fn glider_worn(&mut self, id: PlayerId, output: &mut TickOutput) {
        let chest = clustine_world::ARMOUR_START + 2;
        if let Some(mut random) = self.player_stream(id) {
            let start = random.clone();
            let _slot = random.next_int(1);
            self.player_drew(id, &random, &start);
        }
        self.hurt_and_break_slot(id, chest, 1, output);
    }

    /// [`Region::hurt_and_break_in_hand`] of the hand of `id` that holds `item`.
    pub(crate) fn hurt_and_break_item(
        &mut self,
        id: PlayerId,
        item: i32,
        amount: i32,
        output: &mut TickOutput,
    ) {
        if let Some(hand) = self.hand_with_item(id, item) {
            self.hurt_and_break_in_hand(id, hand, amount, output);
        }
    }

    /// [`Region::hurt_and_break_item`] of the player whose entity is `player`.
    pub(crate) fn hurt_and_break_item_of_entity(
        &mut self,
        player: EntityId,
        item: i32,
        amount: i32,
        output: &mut TickOutput,
    ) {
        if let Some(id) = self.player_with_entity(player) {
            self.hurt_and_break_item(id, item, amount, output);
        }
    }

    /// `ItemUtils.createFilledResult(held, player, filled, prevent_duplicates)` of what
    /// `id` holds in `hand` (n7 ItemUtils.createFilledResult @0-74), set back into the
    /// hand: for a player with infinite materials the hand as it was, and `filled` added
    /// to the inventory (where `prevent_duplicates`, only if none is there yet); for
    /// anyone else one of the hand used up, and `filled` in the hand where that empties
    /// it, else added to the inventory, else dropped at their feet (`Player.drop`).
    pub(crate) fn create_filled_result(
        &mut self,
        id: PlayerId,
        hand: Hand,
        filled: ItemStack,
        prevent_duplicates: bool,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let infinite = player.mode.current.abilities().instabuild;
        if infinite && prevent_duplicates {
            // `Inventory.contains(stack)`: an item of the same kind anywhere.
            if !player
                .inventory
                .0
                .iter()
                .flatten()
                .any(|held| held.same_kind(&filled))
            {
                self.add_to_player_inventory(id, filled, output);
            }
            return;
        }
        let held = self.in_hand(id, hand);
        if !infinite {
            let left = held
                .as_ref()
                .and_then(|stack| (stack.count > 1).then(|| stack.with_count(stack.count - 1)));
            if left.is_none() {
                self.set_hand(id, hand, Some(filled), output);
                return;
            }
            self.set_hand(id, hand, left, output);
        }
        if let Some(rest) = self.add_to_player_inventory(id, filled, output) {
            self.drop_from_player(id, rest, output);
        }
    }

    /// What a use of an item on blocks leaves in `id`'s `hand` where they have no
    /// infinite materials (`placement::Spent`): a bucket filled (`createFilledResult`
    /// of three arguments, which keeps duplicates out only for a creative player), a full
    /// bucket emptied (`getEmptySuccessItem`, n7 BucketItem @207-210), a lily pad placed.
    pub(crate) fn spent(
        &mut self,
        id: PlayerId,
        hand: Hand,
        spent: crate::placement::Spent,
        output: &mut TickOutput,
    ) {
        use crate::placement::Spent;
        if self.has_infinite_materials(id) {
            return;
        }
        match spent {
            Spent::Filled(item) => {
                self.create_filled_result(id, hand, ItemStack::new(item, 1), true, output);
            }
            Spent::Emptied => {
                let empty = ItemStack::new(clustine_data::items::BUCKET, 1);
                self.create_filled_result(id, hand, empty, true, output);
            }
            Spent::One => self.consume_in_hand(id, hand, 1, output),
        }
    }

    /// `player.getInventory().add(stack)` of the region's player `id` (n6 Inventory.add
    /// @9-101), the client told of every slot that changed: `None` where all of it went
    /// in, or where none of it did for a player with infinite materials, whose stack the
    /// game then empties (@48-101: only when the count is what it was); else what found
    /// no room, which is the caller's.
    pub(crate) fn add_to_player_inventory(
        &mut self,
        id: PlayerId,
        stack: ItemStack,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let Some(player) = self.players.get_mut(&id) else {
            return Some(stack);
        };
        let infinite = player.mode.current.abilities().instabuild;
        let selected = usize::from(player.selected_slot);
        let (changed, left) =
            super::items::add_to_inventory(&mut player.inventory, selected, &stack);
        if !changed.is_empty() {
            self.journal.players.insert(id);
        }
        for slot in &changed {
            let now = player.inventory.0[*slot].clone();
            let slot = u8::try_from(*slot).expect("an inventory slot fits a byte");
            output
                .player_events
                .push((id, PlayerEvent::InventorySlot { slot, stack: now }));
        }
        let destroyed = infinite && left == stack.count;
        (left > 0 && !destroyed).then(|| stack.with_count(left))
    }

    /// `Player.drop(stack, false)` of the region's player `id`: thrown from their eyes
    /// as their own drop is.
    pub(crate) fn drop_from_player(
        &mut self,
        id: PlayerId,
        stack: ItemStack,
        output: &mut TickOutput,
    ) {
        let thrown = super::entities::Thrown {
            player: id,
            stack,
            crouching: false,
        };
        self.throw(thrown, output);
    }

    /// The stack in `id`'s `hand` set to `stack`, and their client told.
    pub(crate) fn set_hand(
        &mut self,
        id: PlayerId,
        hand: Hand,
        stack: Option<ItemStack>,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let slot = match hand {
            Hand::Main => usize::from(player.selected_slot),
            Hand::Off => OFF_HAND,
        };
        self.set_inventory_slot(id, slot, stack, output);
    }

    /// The inventory's `slot` of `id` set to `stack`, and their client told.
    pub(crate) fn set_inventory_slot(
        &mut self,
        id: PlayerId,
        slot: usize,
        stack: Option<ItemStack>,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if !player.inventory.set(slot, stack.clone()) {
            return;
        }
        self.journal.players.insert(id);
        let slot = u8::try_from(slot).expect("an inventory slot fits a byte");
        output
            .player_events
            .push((id, PlayerEvent::InventorySlot { slot, stack }));
    }
}
