//! Trading with a villager or a wandering trader in the region (`AbstractVillager`,
//! `Villager.startTrading`, `MerchantMenu`; `docs/groundwork/n10-plan.md`, section 8.2):
//! opening the merchant's window, a trade picked, and the end of trading. Part T2 fills
//! these; the contract step N10c put the calls to them where they are made.

use clustine_world::{EntityId, PlayerId};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// `startTrading(player)`: the merchant `merchant` (a villager or a wandering trader)
    /// opens its window for the player `player`, who used it.
    pub(super) fn start_trading(
        &mut self,
        merchant: EntityId,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        let _ = (merchant, player, output);
    }

    /// `handleSelectTrade`: the player picked the trade `index` in the merchant's menu
    /// of the serial `serial`; refused for a menu that is not a merchant's.
    pub(super) fn select_trade(
        &mut self,
        player: PlayerId,
        serial: u64,
        index: i32,
        output: &mut TickOutput,
    ) {
        let _ = (player, serial, index, output);
    }

    /// `MerchantContainer.updateSellItem`: the payments of the player's merchant menu
    /// changed; its result is worked out again for them.
    pub(super) fn merchant_payments_changed(
        &mut self,
        player: PlayerId,
        merchant: EntityId,
        output: &mut TickOutput,
    ) {
        let _ = (player, merchant, output);
    }

    /// `AbstractVillager.stillValid(player)`, of the merchant `merchant` for a player
    /// whose eyes are at `eyes`: the merchant there, trading with them, and within their
    /// reach.
    pub(super) fn merchant_still_valid(&self, merchant: EntityId, eyes: [f64; 3]) -> bool {
        let _ = eyes;
        self.find_entity(merchant).is_some()
    }

    /// The translation key of the merchant's window's title (`getDisplayName`): a
    /// villager's by its profession, a trader's by its type.
    pub(super) fn merchant_title(&self, merchant: EntityId) -> String {
        let _ = merchant;
        "entity.minecraft.villager".to_owned()
    }

    /// The player's merchant menu on `merchant` closed, for whatever reason: the
    /// payments go back to them (`MerchantMenu.removed`), and the merchant trades with
    /// nobody (`setTradingPlayer(null)`).
    pub(super) fn trading_closed(
        &mut self,
        player: PlayerId,
        merchant: EntityId,
        trade: &mut crate::state::TradeSlots,
        output: &mut TickOutput,
    ) {
        let _ = (player, merchant, trade, output);
    }
}
