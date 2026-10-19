//! What a mob's use gives the player who used it (`ItemUtils.createFilledResult`, the
//! plan's 7.3.6): a milk bucket from a cow, a stew from a mooshroom. Where the player is
//! another region's, who used the mob across a line, it goes to them as an effect at
//! where they stood, and their region gives it.

use clustine_world::{ARMOUR_START, Effect, Effected, EntityId, ItemStack, PlayerId};

use super::Region;
use crate::api::{PlayerEvent, TickOutput};

impl Region {
    /// Gives `stack` to the player whose stay is `player`, who stood at `from` when they
    /// used the mob (their hand keeps what it held: every player is creative).
    pub(crate) fn give_to_player(
        &mut self,
        player: EntityId,
        stack: ItemStack,
        no_duplicates: bool,
        from: [f64; 3],
        output: &mut TickOutput,
    ) {
        if let Some(id) = self.player_with_entity(player) {
            self.take_given(id, stack, no_duplicates, output);
            return;
        }
        let at = crate::physics::containing(from);
        if !self.sends_to(at.chunk()) {
            return;
        }
        self.made.push(Effect {
            at,
            what: Effected::Given {
                player,
                stack,
                no_duplicates,
            },
        });
    }

    /// `ItemUtils.createFilledResult` for the region's creative player `id`: `stack` put
    /// into the first empty slot of the hotbar and the main inventory, unless
    /// `no_duplicates` and a slot holds one already; nowhere if the inventory is full.
    pub(crate) fn take_given(
        &mut self,
        id: PlayerId,
        stack: ItemStack,
        no_duplicates: bool,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let slots = &mut player.inventory.0;
        if no_duplicates && slots.iter().flatten().any(|held| held.item == stack.item) {
            return;
        }
        let Some(slot) = slots[..ARMOUR_START].iter().position(Option::is_none) else {
            return;
        };
        slots[slot] = Some(stack);
        self.journal.players.insert(id);
        let slot = u8::try_from(slot).expect("an inventory slot fits a byte");
        output.player_events.push((
            id,
            PlayerEvent::InventorySlot {
                slot,
                stack: Some(stack),
            },
        ));
    }
}
