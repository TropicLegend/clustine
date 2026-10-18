//! A player's pick of an entity's item (`pick_item_from_entity`, the middle click;
//! `ServerGamePacketListenerImpl.handlePickItemFromEntity` and `tryPickItem`,
//! `docs/groundwork/n5-brief.md`, section 2.9): answered by the player's own region from
//! its record of the entity or its copy of the entity's chunk, never as an effect
//! (`docs/groundwork/n5-plan.md`, section 8 item 7), so a pick across a line is at most
//! a tick behind what the entity holds.
//!
//! The kinds whose pick is built are the frames and paintings (`hanging.rs`) and the
//! knots (`knots.rs`); the other kinds' items come with their own steps and pick
//! nothing until then.

use clustine_world::{EntityId, HOTBAR_SLOTS, ItemStack, MAIN_START, PlayerId};

use super::Region;
use super::hanging::hanging_pick;
use super::knots::knot_pick;
use super::uses::{CREATIVE_ENTITY_REACH, User, distance_to_sqr};
use crate::api::{PlayerEvent, TickOutput};

/// The slots `Inventory.items` has, the hotbar's and the main inventory's, in which a
/// pick looks and puts.
const ITEMS: usize = MAIN_START + 27;

impl Region {
    /// The player `player`, standing as `user`, picked the item of the entity `key`:
    /// where the region has it or a copy of it, within reach (the interaction range and
    /// the 3 the handler adds, from the eyes to its box), its `getPickResult()` is
    /// selected where the inventory has it, or else put into the hotbar, every player
    /// being creative; the client is told the slot selected, and every slot changed.
    pub(crate) fn pick_entity_item(
        &mut self,
        player: PlayerId,
        user: User,
        key: EntityId,
        output: &mut TickOutput,
    ) {
        let entity = match self.find_entity(key) {
            Some((_, entity)) => entity.clone(),
            None => match self.copied_entity(key) {
                Some((_, entity)) => entity,
                None => return,
            },
        };
        let reach = CREATIVE_ENTITY_REACH * CREATIVE_ENTITY_REACH;
        if distance_to_sqr(self.entity_box(&entity), user.eyes()) >= reach {
            return;
        }
        let Some(picked) = hanging_pick(&entity).or_else(|| knot_pick(&entity)) else {
            return;
        };
        let Some(state) = self.players.get_mut(&player) else {
            return;
        };
        let mut slots: Vec<Option<ItemStack>> = state.inventory.0[..ITEMS].to_vec();
        let mut selected = usize::from(state.selected_slot);
        let changed = try_pick(&mut slots, &mut selected, picked, true);
        for &slot in &changed {
            state.inventory.set(slot, slots[slot]);
        }
        state.selected_slot = u8::try_from(selected).expect("a hotbar slot fits a byte");
        // `ClientboundSetHeldSlotPacket`, whether it changed or not, then the menu's
        // changes (`broadcastChanges`), in the order of its slots: the main inventory's
        // before the hotbar's.
        output.player_events.push((
            player,
            PlayerEvent::HeldSlot {
                slot: state.selected_slot,
            },
        ));
        let mut changed = changed;
        changed.sort_by_key(|&slot| (slot < MAIN_START, slot));
        changed.dedup();
        for slot in changed {
            let event = PlayerEvent::InventorySlot {
                slot: u8::try_from(slot).expect("an inventory slot fits a byte"),
                stack: slots[slot],
            };
            output.player_events.push((player, event));
        }
    }
}

/// `tryPickItem(stack)` on the items `slots` (the hotbar and the main inventory) with
/// `selected` the hotbar's selected slot: a slot holding the same item is selected where
/// it is in the hotbar, or swapped into a suitable hotbar slot; else, for a player with
/// infinite materials, the stack is put into a suitable hotbar slot, what was there
/// moving to the first empty slot if there is one. The slots changed.
///
/// **Inferred**: `Inventory.findSlotMatchingItem`, `pickSlot`, `addAndPickItem`,
/// `getSuitableHotbarSlot` and `getFreeSlot` are not in the dump; these are their bodies
/// as the game has had them since picking moved to the server, with Clustine's stacks,
/// which have no components and so match by item alone, and enchanted by nothing.
fn try_pick(
    slots: &mut [Option<ItemStack>],
    selected: &mut usize,
    stack: ItemStack,
    infinite: bool,
) -> Vec<usize> {
    let matching = slots
        .iter()
        .position(|slot| slot.is_some_and(|held| held.item == stack.item));
    match matching {
        Some(slot) if slot < HOTBAR_SLOTS => {
            *selected = slot;
            Vec::new()
        }
        Some(slot) => {
            // `pickSlot`.
            *selected = suitable_hotbar_slot(slots, *selected);
            slots.swap(*selected, slot);
            vec![*selected, slot]
        }
        None if infinite => {
            // `addAndPickItem`.
            *selected = suitable_hotbar_slot(slots, *selected);
            let mut changed = vec![*selected];
            if let Some(there) = slots[*selected]
                && let Some(free) = slots.iter().position(Option::is_none)
            {
                slots[free] = Some(there);
                changed.push(free);
            }
            slots[*selected] = Some(stack);
            changed
        }
        None => Vec::new(),
    }
}

/// `getSuitableHotbarSlot()`: the first empty hotbar slot from the selected one on,
/// round, or else the first not enchanted, which is the selected one.
fn suitable_hotbar_slot(slots: &[Option<ItemStack>], selected: usize) -> usize {
    (0..HOTBAR_SLOTS)
        .map(|offset| (selected + offset) % HOTBAR_SLOTS)
        .find(|&slot| slots[slot].is_none())
        .unwrap_or(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(item: i32) -> Option<ItemStack> {
        Some(ItemStack { item, count: 1 })
    }

    #[test]
    fn a_pick_of_what_the_hotbar_holds_selects_its_slot() {
        let mut slots = vec![None; ITEMS];
        slots[4] = stack(7);
        let mut selected = 0;
        assert!(
            try_pick(
                &mut slots,
                &mut selected,
                ItemStack { item: 7, count: 1 },
                true
            )
            .is_empty()
        );
        assert_eq!(selected, 4);
    }

    #[test]
    fn a_pick_of_what_the_main_inventory_holds_swaps_it_into_an_empty_hotbar_slot() {
        let mut slots = vec![stack(1); ITEMS];
        slots[3] = None;
        slots[20] = stack(7);
        let mut selected = 1;
        let changed = try_pick(
            &mut slots,
            &mut selected,
            ItemStack { item: 7, count: 1 },
            true,
        );
        assert_eq!(selected, 3);
        assert_eq!(changed, [3, 20]);
        assert_eq!(slots[3], stack(7));
        assert_eq!(slots[20], None);
    }

    #[test]
    fn a_creative_pick_of_something_new_goes_into_the_hotbar_and_moves_what_was_there() {
        let mut slots = vec![stack(1); ITEMS];
        slots[30] = None;
        let mut selected = 2;
        let changed = try_pick(
            &mut slots,
            &mut selected,
            ItemStack { item: 7, count: 1 },
            true,
        );
        // No empty hotbar slot: the selected one, its stack moved to the first empty.
        assert_eq!(selected, 2);
        assert_eq!(changed, [2, 30]);
        assert_eq!(slots[2], stack(7));
        assert_eq!(slots[30], stack(1));
        // Without infinite materials nothing happens.
        let mut selected = 0;
        let changed = try_pick(
            &mut slots,
            &mut selected,
            ItemStack { item: 9, count: 1 },
            false,
        );
        assert!(changed.is_empty());
        assert_eq!(selected, 0);
    }
}
