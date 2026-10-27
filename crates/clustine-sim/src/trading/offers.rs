//! Drawing offers from the compiled trades: `VillagerTrade.getOffer` (n10 VillagerTrade
//! 95–239, as `docs/groundwork/n10-dumps.md`, part B, section 5 has it) and
//! `AbstractVillager.addOffersFromTradeSet` (n7 AbstractVillager 387–534).
//!
//! `getOffer`'s order: the merchant's predicate; every modifier on what it gives (an
//! empty result gives no offer); the hidden additional cost taken off the result, doubled
//! only where the result stores an enchantment of the trade's doubling set; then the
//! wanted cost (`TradeCost.toItemCost(extra)`: `clamp(count + extra, 0, the item's max
//! stack)`, no offer below 1), the second cost, the uses (at least 1), the experience (at
//! least 0) and the discount (at least 0), each drawn in that order. In the game's data
//! every count, use, experience and discount is a constant, so only the modifiers draw.

use clustine_data::ItemComponents;
use clustine_data::enchanting;
use clustine_data::trading::{TradeCost, TradeId, TradeSet, VillagerTrade, villager_trade};
use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind};
use clustine_world::trading::{ExactPredicate, ItemCost, Offer};

use super::evaluate::{TradeStack, apply_all, int_value, world_component};
use crate::region::Random;

/// What a trade reads of its merchant (the loot context's `this`): the villager's type
/// (`minecraft:plains`), which a wandering trader has not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MerchantContext<'a> {
    pub(crate) villager_type: Option<&'a str>,
}

/// `TradeCost.toItemCost(ctx, extra)`: its count drawn, `extra` added, kept within 0 and
/// the item's max stack size (TradeCost.txt 85–108).
pub(crate) fn to_item_cost(cost: &TradeCost, extra: i32, random: &mut Random) -> ItemCost {
    let item = i32::from(cost.item);
    let most = i32::from(ItemComponents::of(item).max_stack_size);
    let count = int_value(&cost.count, random)
        .wrapping_add(extra)
        .clamp(0, most);
    ItemCost {
        item,
        count,
        components: ExactPredicate::of(cost.components.iter().map(world_component).collect()),
    }
}

/// `VillagerTrade.getOffer(ctx)`: the offer `trade` makes `merchant`, or `None` where the
/// merchant may not make it or its result comes to nothing.
pub(crate) fn get_offer(
    trade: &VillagerTrade,
    merchant: &MerchantContext<'_>,
    random: &mut Random,
) -> Option<Offer> {
    // 1. `entity_properties` of `this`: the villager's variant among those listed.
    if let Some(predicate) = &trade.merchant_predicate {
        let allowed = merchant
            .villager_type
            .is_some_and(|kind| predicate.villager_variants.contains(&kind));
        if !allowed {
            return None;
        }
    }
    // 2, 3. What it gives, through every modifier.
    let mut given = ItemStack::new(i32::from(trade.gives.item), trade.gives.count);
    for component in trade.gives.components {
        given.components.set(world_component(component));
    }
    let mut result = TradeStack::of(given);
    apply_all(trade.given_item_modifier, &mut result, random);
    let stack = result.stack?;
    // 4. The hidden cost comes off the stack and never reaches the offer.
    let mut extra = result.additional_trade_cost.unwrap_or(0);
    // 5. Doubled only for a stored enchantment of the set (a book's, not a tool's own).
    if let Some(set) = &trade.double_trade_price_enchantments
        && let Some(Component::StoredEnchantments(levels)) =
            stack.components.get(Kind::StoredEnchantments)
        && levels
            .iter()
            .any(|(name, _)| enchanting::enchantment_in(set, name))
    {
        extra = extra.wrapping_mul(2);
    }
    // 6, 7. The costs.
    let cost_a = to_item_cost(&trade.wants, extra, random);
    if cost_a.count < 1 {
        return None;
    }
    let cost_b = match &trade.additional_wants {
        Some(wants) => {
            let cost = to_item_cost(wants, 0, random);
            if cost.count < 1 {
                return None;
            }
            Some(cost)
        }
        None => None,
    };
    // 8. The offer, its numbers floored.
    let max_uses = int_value(&trade.max_uses, random).max(1);
    let xp = int_value(&trade.xp, random).max(0);
    let discount = trade.reputation_discount.max(0.0);
    Some(Offer::new(cost_a, cost_b, stack, max_uses, xp, discount))
}

/// `addOffersFromTradeSet`: `set`'s amount drawn, then trades picked at random until that
/// many have given an offer or none is left. Without duplicates (every set of the game's)
/// each pick is removed whether it gave an offer or not; with them only one that failed
/// is. A trade that gives no offer is not counted.
pub(crate) fn add_offers_from_trade_set(
    set: &TradeSet,
    merchant: &MerchantContext<'_>,
    random: &mut Random,
) -> Vec<Offer> {
    let wanted = int_value(&set.amount, random);
    let mut left: Vec<TradeId> = set.trades.to_vec();
    let mut offers = Vec::new();
    let mut made = 0;
    while made < wanted && !left.is_empty() {
        let length = u32::try_from(left.len()).unwrap_or(u32::MAX);
        let at = random.next_int(length) as usize;
        let trade = villager_trade(left[at]);
        if !set.allow_duplicates {
            left.remove(at);
        }
        match get_offer(trade, merchant, random) {
            Some(offer) => {
                offers.push(offer);
                made += 1;
            }
            None => {
                if set.allow_duplicates {
                    left.remove(at);
                }
            }
        }
    }
    offers
}
