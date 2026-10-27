//! A merchant's rules (`AbstractVillager`, `Villager`'s trading, the wandering trader's;
//! `docs/groundwork/n10-plan.md`, section 8.2; the brief's section 5, as
//! `n10-dumps.md`, part B, sections 4 to 6 settle it): its offers drawn when first asked,
//! the click that opens trading, the prices a player is offered, a trade's use, its
//! experience and the level it may reach, restock and demand, and what a merchant's
//! window works out of the payments it is given. The window and its slots are the
//! region's (`region/trading.rs`); everything here is a function of the merchant and
//! the numbers handed in.
//!
//! The offers are drawn from the stream the caller hands, the merchant's stream aside
//! (decision 5, named): the game draws each set from a sequence of the whole world.
//!
//! Named differences: a restock check of a villager that has never been asked for its
//! offers does not draw them (the game's `needsToRestock` does, from the world's
//! sequences, which no villager's stream follows), so they are drawn at the first click
//! as ever; the prices a restock or a catch-up of demand works out again for a player
//! trading are not, as the brain's tick knows no player's effects (they are at the next
//! opening and at a level reached in a trade).

use clustine_data::trading::{
    WANDERING_TRADER_BUYING, WANDERING_TRADER_COMMON, WANDERING_TRADER_UNCOMMON,
    profession_trade_set, trade_set_by_name,
};
use clustine_data::villagers::{PROFESSION_NAMES, TYPE_NAMES};
use clustine_data::{entity_types, items, sounds};
use clustine_world::trading::Offers;
use clustine_world::{ItemStack, PlayerId};

use super::offers::{MerchantContext, add_offers_from_trade_set};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::body::effects::{self, Effect};
use crate::mobs::body::hurt::mob_voice_pitch;
use crate::mobs::types::{self, Did, Interaction, Used, animal, villager, wandering_trader};
use crate::region::Random;

/// `VillagerData.NEXT_LEVEL_XP_THRESHOLDS`: the experience a villager of each level
/// needs for the next (`getMaxXpPerLevel`).
const NEXT_LEVEL_XP: [i32; 5] = [0, 10, 70, 150, 250];

/// A villager's highest level (`VillagerData.MAX_VILLAGER_LEVEL`).
const MAX_LEVEL: i32 = 5;

/// `Villager.restock`'s spacing: a second restock of a day waits 2400 ticks after the
/// first (`allowedToRestock`), and a gap of 12000 since the last starts the count again
/// (`shouldRestock`).
const RESTOCK_SPACING: i64 = 2400;
const RESTOCK_RESET: i64 = 12_000;

/// How many times a villager restocks in a day.
const RESTOCKS_A_DAY: i32 = 2;

/// The ticks of a day of the overworld's clock (`OVERWORLD_DAY`'s period).
pub(crate) const DAY_TICKS: u64 = 24_000;

/// Whether an entity of the type `kind` is a villager (the other merchant is the
/// wandering trader).
pub(crate) fn is_villager(kind: u16) -> bool {
    i32::from(kind) == entity_types::VILLAGER
}

/// Whether an entity of the type `kind` is a merchant: a villager or a wandering trader.
pub(crate) fn is_merchant(kind: u16) -> bool {
    is_villager(kind) || i32::from(kind) == entity_types::WANDERING_TRADER
}

/// The merchant's offers, none before they are first asked for (`offers`).
pub(crate) fn offers(mob: &Mob, kind: u16) -> Option<Offers> {
    if is_villager(kind) {
        villager::state_of(mob).1.offers
    } else {
        wandering_trader::trading_of(mob).offers
    }
}

/// Sets the merchant's offers.
pub(crate) fn set_offers(mob: &mut Mob, kind: u16, offers: Option<Offers>) {
    if is_villager(kind) {
        villager::update(mob, |own| own.offers = offers);
    } else {
        wandering_trader::update_trading(mob, |own| own.offers = offers);
    }
}

/// The player the merchant trades with (`getTradingPlayer`).
pub(crate) fn trading_player(mob: &Mob, kind: u16) -> Option<PlayerId> {
    if is_villager(kind) {
        villager::state_of(mob).1.trading_player
    } else {
        wandering_trader::trading_of(mob).trading_player
    }
}

/// `setTradingPlayer(player)`. A villager's `stopTrading` on letting a player go does
/// nothing more: its prices stay the last trader's until they are worked out again
/// (the brief's 5.4).
pub(crate) fn set_trading_player(mob: &mut Mob, kind: u16, player: Option<PlayerId>) {
    if is_villager(kind) {
        villager::update(mob, |own| own.trading_player = player);
    } else {
        wandering_trader::update_trading(mob, |own| own.trading_player = player);
    }
}

/// What the merchant's window shows beside the offers (`sendMerchantOffers`'s
/// arguments): its level, its experience, whether the client shows the progress bar and
/// whether it says the merchant restocks. A trader is always of level 1, gains no
/// experience and restocks never (`openTradingScreen(player, name, 1)`).
pub(crate) fn window_facts(mob: &Mob, kind: u16) -> (i32, i32, bool, bool) {
    if is_villager(kind) {
        let own = villager::state_of(mob).1;
        (own.data.level, own.xp, true, true)
    } else {
        (1, 0, false, false)
    }
}

/// `getOffers()`: the merchant's offers, drawn from `random` (`updateTrades`) the first
/// time they are asked for and kept.
pub(crate) fn get_offers(mob: &mut Mob, kind: u16, random: &mut Random) -> Offers {
    if let Some(offers) = offers(mob, kind) {
        return offers;
    }
    let mut offers = Offers::default();
    if is_villager(kind) {
        update_villager_trades(mob, &mut offers, random);
    } else {
        update_trader_trades(&mut offers, random);
    }
    set_offers(mob, kind, Some(offers.clone()));
    offers
}

/// `Villager.updateTrades` (1666–1707): the trade set of its profession at its level
/// (none for the unemployed and the nitwit) drawn onto `offers`.
fn update_villager_trades(mob: &Mob, offers: &mut Offers, random: &mut Random) {
    let data = villager::state_of(mob).1.data;
    let Some(profession) = PROFESSION_NAMES.get(usize::from(data.profession)) else {
        return;
    };
    let Some(set) = profession_trade_set(profession, data.level) else {
        return;
    };
    let merchant = MerchantContext {
        villager_type: TYPE_NAMES.get(usize::from(data.kind)).copied(),
    };
    offers
        .0
        .extend(add_offers_from_trade_set(set, &merchant, random));
}

/// `WanderingTrader.updateTrades` (282–303): what it buys, then its uncommon and its
/// common wares, each set drawn in turn.
fn update_trader_trades(offers: &mut Offers, random: &mut Random) {
    let merchant = MerchantContext {
        villager_type: None,
    };
    for name in [
        WANDERING_TRADER_BUYING,
        WANDERING_TRADER_UNCOMMON,
        WANDERING_TRADER_COMMON,
    ] {
        if let Some(set) = trade_set_by_name(name) {
            offers
                .0
                .extend(add_offers_from_trade_set(set, &merchant, random));
        }
    }
}

/// `Villager.updateSpecialPrices(player)` (645–749): every special price set back, then
/// each offer made cheaper by `floor(reputation × multiplier)` (in floats) and, for a
/// hero of the village of the amplifier `hero`, by at least one of
/// `floor((0.3 + 0.0625 × amplifier) × base cost)` (the factor a float made a double).
pub(crate) fn update_special_prices(offers: &mut Offers, reputation: i32, hero: Option<u8>) {
    let factor = hero.map_or(0.0, |amplifier| {
        f64::from(0.3_f32 + 0.0625_f32 * f32::from(amplifier))
    });
    for offer in &mut offers.0 {
        offer.reset_special_price_diff();
    }
    for offer in &mut offers.0 {
        if reputation != 0 {
            // `Mth.floor(float)` of the product of two floats.
            let off = ((reputation as f32) * offer.price_multiplier.0).floor() as i32;
            offer.add_to_special_price_diff(off.wrapping_neg());
        }
        if factor > 0.0 {
            let off = (factor * f64::from(offer.cost_a.count)).floor() as i32;
            offer.add_to_special_price_diff(off.max(1).wrapping_neg());
        }
    }
}

/// `MerchantOffers.getRecipeFor(a, b, hint)` (n7k MerchantOffers 27–70): with a hint
/// above nought that names an offer, that offer alone, if `a` and `b` pay it; else the
/// first offer they pay.
pub(crate) fn recipe_for(
    offers: &Offers,
    a: Option<&ItemStack>,
    b: Option<&ItemStack>,
    hint: i32,
) -> Option<usize> {
    if let Ok(at) = usize::try_from(hint)
        && hint > 0
        && at < offers.0.len()
    {
        return offers.0[at].satisfied_by(a, b).then_some(at);
    }
    offers.0.iter().position(|offer| offer.satisfied_by(a, b))
}

/// What `MerchantContainer.updateSellItem` (n10 MerchantContainer 163–267) works out of
/// the two payments: the active offer, the result, and whether the merchant is told
/// (`notifyTradeUpdated`, which it is not where nothing is paid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SellItem {
    pub(crate) active: Option<usize>,
    pub(crate) result: Option<ItemStack>,
    pub(crate) notify: bool,
}

/// `updateSellItem`: with the first payment empty the second counts as the first; the
/// offer they pay, else the one they pay the other way round, not out of stock, gives
/// its result. Where the first way finds an offer out of stock and the second none, the
/// active offer stays that one, with no result.
pub(crate) fn sell_item(offers: &Offers, payments: &[Option<ItemStack>; 2], hint: i32) -> SellItem {
    let paid = |stack: &Option<ItemStack>| stack.clone().filter(|stack| stack.count > 0);
    let (a, b) = match paid(&payments[0]) {
        None => (paid(&payments[1]), None),
        Some(first) => (Some(first), paid(&payments[1])),
    };
    if a.is_none() {
        return SellItem {
            active: None,
            result: None,
            notify: false,
        };
    }
    let mut active = None;
    let mut result = None;
    if !offers.0.is_empty() {
        let in_stock = |at: &usize| !offers.0[*at].is_out_of_stock();
        let mut found = recipe_for(offers, a.as_ref(), b.as_ref(), hint);
        if !found.as_ref().is_some_and(in_stock) {
            active = found;
            found = recipe_for(offers, b.as_ref(), a.as_ref(), hint);
        }
        if let Some(at) = found.filter(in_stock) {
            active = Some(at);
            result = Some(offers.0[at].assemble());
        }
    }
    SellItem {
        active,
        result,
        notify: true,
    }
}

/// `MerchantResultSlot.mayPickup`'s test: an active offer that the payments pay, either
/// way round.
pub(crate) fn may_take(offers: &Offers, payments: &[Option<ItemStack>; 2], hint: i32) -> bool {
    let Some(at) = sell_item(offers, payments, hint).active else {
        return false;
    };
    let offer = &offers.0[at];
    let [a, b] = payments;
    offer.satisfied_by(a.as_ref(), b.as_ref()) || offer.satisfied_by(b.as_ref(), a.as_ref())
}

/// What a trade did beyond the merchant: the experience orb it drops (at its feet, half
/// a block up), and whether a villager reached a level, which shows the window its new
/// offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Traded {
    pub(crate) orb: Option<i32>,
    pub(crate) levelled: bool,
}

/// `notifyTrade(offer)` of the offer `at` (`AbstractVillager` 153–177): its use counted,
/// the ambient sound put off its whole interval (`ambientSoundTime = −interval`, at the
/// moment `moment`), and the trade's experience given (`rewardTradeXp`). `random` is the
/// merchant's own; `player` the trading player, whom a villager remembers for the
/// trade's reputation event (`lastTradedPlayer`). A villager that reaches a level draws
/// that level's offers from `random` and has its prices worked out again for
/// `reputation` and `hero` (`updateTrades`' `updateSpecialPrices`).
#[expect(
    clippy::too_many_arguments,
    reason = "the trade's parts, as the game has them"
)]
pub(crate) fn notify_trade(
    mob: &mut Mob,
    kind: u16,
    at: usize,
    random: &mut Random,
    moment: u32,
    player: Option<PlayerId>,
    reputation: i32,
    hero: Option<u8>,
) -> Traded {
    let Some(mut offers) = offers(mob, kind) else {
        return Traded::default();
    };
    let Some(offer) = offers.0.get_mut(at) else {
        return Traded::default();
    };
    offer.increase_uses();
    let offer = offer.clone();
    set_offers(mob, kind, Some(offers));
    mob.body.idle.ambient_since = moment.wrapping_add(types::of(kind).ambient_interval());
    if is_villager(kind) {
        reward_villager(mob, kind, &offer, random, moment, player, reputation, hero)
    } else {
        // `WanderingTrader.rewardTradeXp` (355–386): an orb of `3 + nextInt(4)` where
        // the offer rewards experience, and nothing else.
        let orb = offer.reward_exp.then(|| 3 + random.next_int(4) as i32);
        Traded {
            orb,
            levelled: false,
        }
    }
}

/// `Villager.rewardTradeXp` (1025–1091): `3 + nextInt(4)` drawn, the offer's experience
/// added, the trading player remembered, and where the next level's threshold is
/// reached the next level (`increaseMerchantCareer`: its trades drawn, its prices for
/// the trading player), 200 ticks of regeneration and five more for the orb, which the
/// offer drops where it rewards experience. One level a trade.
#[expect(
    clippy::too_many_arguments,
    reason = "the trade's parts, as the game has them"
)]
fn reward_villager(
    mob: &mut Mob,
    kind: u16,
    offer: &clustine_world::trading::Offer,
    random: &mut Random,
    moment: u32,
    player: Option<PlayerId>,
    reputation: i32,
    hero: Option<u8>,
) -> Traded {
    let mut orb = 3 + random.next_int(4) as i32;
    villager::update(mob, |own| {
        own.xp = own.xp.wrapping_add(offer.xp);
        own.last_traded_player = player;
    });
    let own = villager::state_of(mob).1;
    let level = own.data.level;
    let levelled = (1..MAX_LEVEL).contains(&level)
        && own.xp >= NEXT_LEVEL_XP[usize::try_from(level).unwrap_or(0)];
    if levelled {
        // `setVillagerData(withLevel(level + 1))`: the same profession, so the offers
        // are kept, and the new level's added to them.
        villager::update(mob, |own| own.data.level = level + 1);
        let mut offers = offers(mob, kind).unwrap_or_default();
        update_villager_trades(mob, &mut offers, random);
        if player.is_some() {
            update_special_prices(&mut offers, reputation, hero);
        }
        set_offers(mob, kind, Some(offers));
        effects::add(mob, kind, Effect::REGENERATION, 0, 200, moment);
        orb += 5;
    }
    Traded {
        orb: offer.reward_exp.then_some(orb),
        levelled,
    }
}

/// `notifyTradeUpdated(result)` (`AbstractVillager` 186–215), told of the result a
/// window's payments make at the moment `moment`: where its ambient sound has waited at
/// least 20 ticks of its interval, it is put off again, and the merchant says yes to a
/// result or no to none, at its voice drawn of `random`. The sound and its pitch, if it
/// says anything.
pub(crate) fn notify_trade_updated(
    mob: &mut Mob,
    kind: u16,
    moment: u32,
    result: bool,
    random: &mut Random,
) -> Option<(i32, f32)> {
    let mob_type = types::of(kind);
    let interval = mob_type.ambient_interval();
    let time = i64::from(moment) - i64::from(mob.body.idle.ambient_since);
    if time <= 20 - i64::from(interval) {
        return None;
    }
    mob.body.idle.ambient_since = moment.wrapping_add(interval);
    let sound = match (is_villager(kind), result) {
        (true, true) => sounds::ENTITY_VILLAGER_YES,
        (true, false) => sounds::ENTITY_VILLAGER_NO,
        (false, true) => sounds::ENTITY_WANDERING_TRADER_YES,
        (false, false) => sounds::ENTITY_WANDERING_TRADER_NO,
    };
    Some((sound, mob_voice_pitch(mob_type, mob, random)))
}

/// `Villager.mobInteract` (279–337): with anything but its egg, a living villager that
/// neither trades nor sleeps shakes its head at a player if it is a baby; a grown one is
/// asked for its offers (drawn now if never before), shakes its head where it has none
/// (the use consumed), and else starts trading with the player (`startTrading`, which
/// the region does: its window is the region's). Otherwise `AgeableMob`'s.
///
/// Named: the statistic of a talk is not kept, and a villager shakes its head at a use
/// of the off hand as of the main one (the use has no hand).
pub(crate) fn villager_interact(used: &mut Used<'_>) -> Interaction {
    let egg = used
        .held
        .as_ref()
        .is_some_and(|stack| stack.count > 0 && stack.item == items::VILLAGER_SPAWN_EGG);
    let own = villager::state_of(used.mob).1;
    if !egg && !used.mob.body.vitals.dying() && own.trading_player.is_none() && !own.shown.sleeping
    {
        if animal::is_baby(used.mob) {
            villager::set_unhappy(used);
            return Interaction::Success;
        }
        let offers = get_offers(used.mob, used.kind, used.draws);
        if offers.0.is_empty() {
            villager::set_unhappy(used);
            return Interaction::Consume;
        }
        used.did.push(Did::StartTrading);
        return Interaction::Success;
    }
    animal::mob_interact(used, Items::Of(&[]), None)
}

/// `WanderingTrader.mobInteract` (230–281): with anything but a villager's egg, a living
/// grown trader that trades with nobody is asked for its offers (drawn now if never
/// before) and, having any, starts trading with the player (the region opens the
/// window); the use is consumed where it has none. Otherwise `AgeableMob`'s.
pub(crate) fn trader_interact(used: &mut Used<'_>) -> Interaction {
    let egg = used
        .held
        .as_ref()
        .is_some_and(|stack| stack.count > 0 && stack.item == items::VILLAGER_SPAWN_EGG);
    let trading = wandering_trader::trading_of(used.mob)
        .trading_player
        .is_some();
    if !egg && !used.mob.body.vitals.dying() && !trading && !animal::is_baby(used.mob) {
        let offers = get_offers(used.mob, used.kind, used.draws);
        if offers.0.is_empty() {
            return Interaction::Consume;
        }
        used.did.push(Did::StartTrading);
        return Interaction::SuccessServer;
    }
    animal::mob_interact(used, Items::Of(&[]), None)
}

/// `WorkAtPoi.start`'s `if (villager.shouldRestock(level)) villager.restock()`, of the
/// villager `mob` at the game time `game_time`, the overworld's clock at `day_ticks`:
/// whether it restocked.
///
/// `shouldRestock` (514–584): a gap of more than 12000 ticks since its last restock, or
/// a new day of the overworld since its last check, starts its count again, after
/// making up the demand of the restocks it missed (`resetNumberOfRestocks`,
/// `catchUpDemand`); it then restocks if it may (none yet today, or one and 2400 ticks
/// since) and an offer was used. `restock` (430–468): every offer's demand updated and
/// its uses set back, the time kept and the count raised.
pub(crate) fn restock_if_due(mob: &mut Mob, game_time: i64, day_ticks: u64) -> bool {
    let day = i64::try_from(day_ticks / DAY_TICKS).unwrap_or(i64::MAX);
    let mut own = villager::state_of(mob).1;
    let mut reset = game_time > own.last_restock.saturating_add(RESTOCK_RESET);
    reset |= own.last_restock_check_day > 0 && day > own.last_restock_check_day;
    own.last_restock_check_day = day;
    if reset {
        own.last_restock = game_time;
        catch_up_demand(&mut own);
        own.restocks_today = 0;
    }
    let allowed = own.restocks_today == 0
        || (own.restocks_today < RESTOCKS_A_DAY
            && game_time > own.last_restock.saturating_add(RESTOCK_SPACING));
    let needed = own
        .offers
        .as_ref()
        .is_some_and(|offers| offers.0.iter().any(|offer| offer.needs_restock()));
    let due = allowed && needed;
    if due {
        if let Some(offers) = own.offers.as_mut() {
            for offer in &mut offers.0 {
                offer.update_demand();
            }
            for offer in &mut offers.0 {
                offer.reset_uses();
            }
        }
        own.last_restock = game_time;
        own.restocks_today = own.restocks_today.wrapping_add(1);
    }
    villager::update(mob, |kept| *kept = own);
    due
}

/// `catchUpDemand` (585–626): for each of the day's restocks it missed, its uses set back
/// and its demand updated once more.
fn catch_up_demand(own: &mut villager::VillagerState) {
    let missed = RESTOCKS_A_DAY - own.restocks_today;
    if missed <= 0 {
        return;
    }
    let Some(offers) = own.offers.as_mut() else {
        return;
    };
    for offer in &mut offers.0 {
        offer.reset_uses();
    }
    for _ in 0..missed {
        for offer in &mut offers.0 {
            offer.update_demand();
        }
    }
}

/// The translation key of the merchant's name (`getDisplayName`'s type name): a
/// villager's by its profession (`VillagerProfession.name()`,
/// `entity.minecraft.villager.<profession>`), a trader's by its type.
pub(crate) fn title(mob: &Mob, kind: u16) -> String {
    if !is_villager(kind) {
        return "entity.minecraft.wandering_trader".to_owned();
    }
    let data = villager::state_of(mob).1.data;
    let profession = PROFESSION_NAMES
        .get(usize::from(data.profession))
        .copied()
        .unwrap_or("minecraft:none");
    let (space, path) = profession
        .split_once(':')
        .unwrap_or(("minecraft", profession));
    format!("entity.{space}.villager.{path}")
}

#[cfg(test)]
#[path = "merchant_n10_tests.rs"]
mod merchant_n10_tests;
