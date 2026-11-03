//! An operator's orders for a player (V1, `docs/groundwork/v-plan.md`, section 4.1):
//! `clustine gamemode` and `clustine give`, or the single binary's standard input, which
//! the edge that holds the player passes into their inputs ([`PlayerInput::Order`]).
//!
//! A change of mode is `ServerPlayer.setGameMode` (n5 @0-92) and
//! `ServerPlayerGameMode.changeGameModeForPlayer`: nothing if it is the mode they have;
//! else the old becomes the previous, the abilities follow the new one (a survival
//! player does not fly), their client is told (the edge sends the abilities, the list's
//! update, the game event and the abilities again, in the game's order) and so is
//! everyone who lists them. A gift is `GiveCommand.giveItem`: stacks of at most the
//! item's largest, each put in the inventory as `Inventory.add` puts it, with the pickup
//! sound when it all went in, else what found no room dropped from the player.
//!
//! Named differences: no `gameMode.changed` message to the player (it needs a
//! translatable argument the protocol does not carry yet); the impulse context
//! creative resets and spectator's camera are not kept (no impulse fall damage, no
//! spectator); `updateSleepingPlayerList` has nothing to count until spectator exists; a
//! gift that went in whole makes no fake item for a tick and so no swing of the arm;
//! what is dropped has a throw's pick-up delay and no target (anyone may take it, where
//! the game's has none and only the player may).

use clustine_data::sounds;
use clustine_world::{GameMode, PlayerId};

use super::Region;
use crate::api::{ItemStack, Order, PlayerEvent, RegionEvent, SOUND_SOURCE_PLAYERS, TickOutput};

impl Region {
    /// Carries out the operator's `order` for the region's player `id`.
    pub(super) fn obey(&mut self, id: PlayerId, order: &Order, output: &mut TickOutput) {
        match *order {
            Order::GameMode(mode) => self.set_game_mode(id, mode, output),
            Order::Give { item, count } => self.give(id, item, count, output),
        }
    }

    fn set_game_mode(&mut self, id: PlayerId, mode: GameMode, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if player.mode.current == mode {
            return;
        }
        player.mode = player.mode.changed_to(mode);
        // `GameType.updatePlayerAbilities`: who may not fly does not.
        if !mode.abilities().may_fly {
            player.flying = false;
        }
        self.journal.players.insert(id);
        output.player_events.push((
            id,
            PlayerEvent::GameModeChanged {
                mode: player.mode,
                flying: player.flying,
            },
        ));
        output.events.push(RegionEvent::PlayerGameMode {
            entity: player.entity_id,
            chunk: player.chunk(),
            player: id,
            mode: mode.id(),
        });
    }

    fn give(&mut self, id: PlayerId, item: i32, count: i32, output: &mut TickOutput) {
        let Ok(index) = usize::try_from(item) else {
            return;
        };
        if item == 0 || index >= clustine_data::ITEMS.len() || count < 1 {
            return;
        }
        let largest = i32::from(clustine_data::ItemComponents::of(item).max_stack_size).max(1);
        let mut left = count;
        while left > 0 {
            let this = left.min(largest);
            left -= this;
            let stack = ItemStack::new(item, this);
            match self.add_to_player_inventory(id, stack, output) {
                None => self.pickup_sound(id, output),
                Some(rest) => self.drop_from_player(id, rest, output),
            }
        }
    }

    /// `GiveCommand`'s pickup sound at the player, to everyone: 0.2 loud, its pitch
    /// `((a − b) · 0.7 + 1) · 2` of two of the player's own draws.
    fn pickup_sound(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let (entity, position) = (player.entity_id, player.pose.position);
        let [a, b] = self.draw_floats::<2>(entity);
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_ITEM_PICKUP,
            source: SOUND_SOURCE_PLAYERS,
            position,
            volume: 0.2,
            pitch: ((a - b) * 0.7 + 1.0) * 2.0,
            except: None,
        });
    }
}
