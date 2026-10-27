//! The merchant's rules from the brief's section 5 and `n10-dumps.md`, part B, sections
//! 4 to 6: offers drawn when first asked, prices for a reputation and a hero, the
//! window's result for its payments, a trade's experience and level, and restock and
//! demand. The numbers are worked out here from the game's formulas.

use clustine_data::trading::{profession_trade_set, trade_set_by_name};
use clustine_data::villagers::professions;
use clustine_data::{entity_types, items, sounds};
use clustine_world::trading::{ExactPredicate, ItemCost, Offer, Offers};
use clustine_world::{EntityId, ItemStack, PlayerId};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect;
use crate::mobs::types::villager;
use crate::region::Random;

const VILLAGER: u16 = entity_types::VILLAGER as u16;
const TRADER: u16 = entity_types::WANDERING_TRADER as u16;

fn random() -> Random {
    Random::of_entity_aside(0x5EED, EntityId(9), 3, 40)
}

fn cost(item: i32, count: i32) -> ItemCost {
    ItemCost {
        item,
        count,
        components: ExactPredicate::EMPTY,
    }
}

/// Twenty wheat for an emerald, sixteen times, two experience, at 0.05.
fn wheat_for_an_emerald() -> Offer {
    Offer::new(
        cost(items::WHEAT, 20),
        None,
        ItemStack::new(items::EMERALD, 1),
        16,
        2,
        0.05,
    )
}

/// A villager of `profession` at `level` with `xp` and `offers`.
fn villager_with(profession: i32, level: i32, xp: i32, offers: Option<Offers>) -> Mob {
    let mut mob = Mob::fresh();
    villager::update(&mut mob, |own| {
        own.data.profession = profession as u8;
        own.data.level = level;
        own.xp = xp;
        own.offers = offers;
    });
    mob
}

fn player(n: u128) -> PlayerId {
    PlayerId(uuid::Uuid::from_u128(n))
}

/// `getOffers()` draws a villager's offers once, the trade set of its profession and
/// level from the stream it is handed, and keeps them; asked again it draws nothing.
#[test]
fn a_villager_s_offers_are_drawn_once_from_its_profession_s_set_at_its_level() {
    let mut mob = villager_with(professions::LIBRARIAN, 1, 0, None);
    let mut drawn = random();
    let offers = get_offers(&mut mob, VILLAGER, &mut drawn);
    let set = profession_trade_set("minecraft:librarian", 1).expect("the librarian's first set");
    let mut replay = random();
    let merchant = super::super::offers::MerchantContext {
        villager_type: Some("minecraft:plains"),
    };
    let expected = add_offers_from_trade_set(set, &merchant, &mut replay);
    assert_eq!(offers.0, expected);
    assert_eq!(offers.0.len(), 2, "the librarian's first level has two");
    assert_eq!(drawn, replay);
    let again = get_offers(&mut mob, VILLAGER, &mut drawn);
    assert_eq!(again, offers);
    assert_eq!(drawn, replay, "nothing drawn the second time");
    // An unemployed villager has none, and keeps the empty list.
    let mut idle = villager_with(professions::NONE, 1, 0, None);
    assert!(get_offers(&mut idle, VILLAGER, &mut random()).0.is_empty());
    assert_eq!(offers_of(&idle), Some(Offers::default()));
}

fn offers_of(mob: &Mob) -> Option<Offers> {
    offers(mob, VILLAGER)
}

/// The wandering trader draws two of what it buys, two uncommon wares and five common
/// ones, the sets in that order from one stream.
#[test]
fn a_trader_s_offers_are_its_three_sets_drawn_in_turn() {
    let mut mob = Mob::fresh();
    let mut drawn = random();
    let offers = get_offers(&mut mob, TRADER, &mut drawn);
    let merchant = super::super::offers::MerchantContext {
        villager_type: None,
    };
    let mut replay = random();
    let mut expected = Vec::new();
    for name in [
        "minecraft:wandering_trader/buying",
        "minecraft:wandering_trader/uncommon",
        "minecraft:wandering_trader/common",
    ] {
        let set = trade_set_by_name(name).expect("the trader's set");
        expected.extend(add_offers_from_trade_set(set, &merchant, &mut replay));
    }
    assert_eq!(offers.0, expected);
    assert_eq!(offers.0.len(), 9);
    assert_eq!(window_facts(&mob, TRADER), (1, 0, false, false));
    assert_eq!(title(&mob, TRADER), "entity.minecraft.wandering_trader");
}

/// `updateSpecialPrices`: `floor(reputation × multiplier)` off in floats, and for a hero
/// `max(1, floor((0.3 + 0.0625 × amplifier) × base))` more, the special prices set back
/// first. The brief's 5.4: base 4, demand 3, multiplier 0.2, reputation 25 costs one.
#[test]
fn prices_fall_by_reputation_and_by_a_hero_s_share_of_the_base_cost() {
    let mut book = Offer::new(
        cost(items::EMERALD, 4),
        Some(cost(items::BOOK, 1)),
        ItemStack::new(items::BOOKSHELF, 1),
        12,
        1,
        0.2,
    );
    book.demand = 3;
    book.special_price = 99;
    let mut offers = Offers(vec![book, wheat_for_an_emerald()]);
    update_special_prices(&mut offers, 25, None);
    assert_eq!(offers.0[0].special_price, -5);
    assert_eq!(offers.0[0].modified_cost_count(), 1);
    // floor(25 × 0.05f) = floor(1.25) = 1.
    assert_eq!(offers.0[1].special_price, -1);
    // A hero of amplifier 0: floor(0.3f × 4) = 1 and floor(0.3f × 20) = 6 more.
    update_special_prices(&mut offers, 25, Some(0));
    assert_eq!(offers.0[0].special_price, -6);
    assert_eq!(offers.0[1].special_price, -7);
    // Of amplifier 4: (0.3 + 0.25) × 20 = 11.
    update_special_prices(&mut offers, 0, Some(4));
    assert_eq!(offers.0[0].special_price, -2);
    assert_eq!(offers.0[1].special_price, -11);
    // A hero always saves one, however cheap the base.
    let mut cheap = Offers(vec![Offer::new(
        cost(items::EMERALD, 1),
        None,
        ItemStack::new(items::BREAD, 6),
        16,
        1,
        0.05,
    )]);
    update_special_prices(&mut cheap, 0, Some(0));
    assert_eq!(cheap.0[0].special_price, -1);
    // A bad reputation raises the price: floor(-100 × 0.05f) = -5.
    update_special_prices(&mut cheap, -100, None);
    assert_eq!(cheap.0[0].special_price, 5);
    assert_eq!(cheap.0[0].modified_cost_count(), 6);
}

/// `updateSellItem`: with the first payment empty the second is taken as the first; an
/// offer is found either way round; one out of stock gives no result but stays active
/// where nothing else is found; nothing paid tells the merchant nothing.
#[test]
fn the_window_s_result_is_the_offer_its_payments_pay_either_way_round() {
    let mut book = Offer::new(
        cost(items::EMERALD, 5),
        Some(cost(items::BOOK, 1)),
        ItemStack::new(items::BOOKSHELF, 1),
        12,
        1,
        0.05,
    );
    let offers = Offers(vec![wheat_for_an_emerald(), book.clone()]);
    let wheat = Some(ItemStack::new(items::WHEAT, 20));
    let sold = sell_item(&offers, &[None, wheat.clone()], 0);
    assert_eq!(sold.active, Some(0));
    assert_eq!(sold.result, Some(ItemStack::new(items::EMERALD, 1)));
    assert!(sold.notify);
    let emeralds = Some(ItemStack::new(items::EMERALD, 5));
    let books = Some(ItemStack::new(items::BOOK, 1));
    assert_eq!(
        sell_item(&offers, &[books.clone(), emeralds.clone()], 0).active,
        Some(1),
        "the other way round"
    );
    assert!(may_take(&offers, &[books.clone(), emeralds.clone()], 0));
    // Out of stock: active, no result.
    book.set_to_out_of_stock();
    let offers = Offers(vec![wheat_for_an_emerald(), book]);
    let sold = sell_item(&offers, &[emeralds.clone(), books.clone()], 0);
    assert_eq!((sold.active, sold.result), (Some(1), None));
    // The game's `mayPickup` asks nothing of the stock; the result is empty anyway.
    assert!(may_take(&offers, &[emeralds, books], 0));
    // Nothing paid: no result, and the merchant is not told.
    let sold = sell_item(&offers, &[None, None], 0);
    assert_eq!((sold.active, sold.result, sold.notify), (None, None, false));
    // Too little paid: told of no result.
    let sold = sell_item(&offers, &[Some(ItemStack::new(items::WHEAT, 19)), None], 0);
    assert_eq!((sold.active, sold.result, sold.notify), (None, None, true));
}

/// `getRecipeFor`'s hint: above nought it tests only the offer it names; at nought, or
/// past the last, every offer in order.
#[test]
fn a_hint_above_nought_tests_only_its_own_offer() {
    let offers = Offers(vec![wheat_for_an_emerald(), wheat_for_an_emerald()]);
    let wheat = ItemStack::new(items::WHEAT, 20);
    assert_eq!(recipe_for(&offers, Some(&wheat), None, 0), Some(0));
    assert_eq!(recipe_for(&offers, Some(&wheat), None, 1), Some(1));
    assert_eq!(recipe_for(&offers, Some(&wheat), None, 7), Some(0));
    let other = Offers(vec![
        wheat_for_an_emerald(),
        Offer::new(
            cost(items::BOOK, 1),
            None,
            ItemStack::new(items::EMERALD, 1),
            12,
            1,
            0.05,
        ),
    ]);
    assert_eq!(recipe_for(&other, Some(&wheat), None, 1), None);
}

/// `rewardTradeXp`: `3 + nextInt(4)` first; the offer's experience added; at the next
/// level's threshold the level rises at once, the new level's set drawn after the orb's
/// number, regeneration for 200 ticks and five more for the orb. The use is counted and
/// the ambient sound put off.
#[test]
fn a_trade_that_reaches_the_threshold_raises_the_level_at_once() {
    let offers = Offers(vec![wheat_for_an_emerald()]);
    let mut mob = villager_with(professions::FARMER, 1, 8, Some(offers));
    let mut drawn = random();
    let traded = notify_trade(
        &mut mob,
        VILLAGER,
        0,
        &mut drawn,
        300,
        Some(player(1)),
        0,
        None,
    );
    let mut replay = random();
    let orb = 3 + replay.next_int(4) as i32 + 5;
    let set = profession_trade_set("minecraft:farmer", 2).expect("the farmer's second set");
    let merchant = super::super::offers::MerchantContext {
        villager_type: Some("minecraft:plains"),
    };
    let new = add_offers_from_trade_set(set, &merchant, &mut replay);
    assert_eq!(drawn, replay);
    assert_eq!(
        traded,
        Traded {
            orb: Some(orb),
            levelled: true
        }
    );
    let own = villager::state_of(&mob).1;
    assert_eq!((own.data.level, own.xp), (2, 10));
    assert_eq!(own.last_traded_player, Some(player(1)));
    let kept = own.offers.expect("offers");
    assert_eq!(kept.0.len(), 1 + new.len());
    assert_eq!(kept.0[0].uses, 1);
    assert_eq!(&kept.0[1..], &new[..]);
    assert!(mob.body.effects.has(Effect::REGENERATION));
    assert_eq!(mob.body.idle.ambient_since, 300 + 80);
    // Below the threshold: no level, no regeneration, the orb its plain number.
    let mut mob = villager_with(
        professions::FARMER,
        1,
        0,
        Some(Offers(vec![wheat_for_an_emerald()])),
    );
    let mut drawn = random();
    let traded = notify_trade(
        &mut mob,
        VILLAGER,
        0,
        &mut drawn,
        0,
        Some(player(1)),
        0,
        None,
    );
    let mut replay = random();
    assert_eq!(traded.orb, Some(3 + replay.next_int(4) as i32));
    assert!(!traded.levelled);
    assert_eq!(drawn, replay);
    assert_eq!(villager::state_of(&mob).1.data.level, 1);
    // A master (level 5) never rises.
    let mut master = villager_with(
        professions::FARMER,
        5,
        1000,
        Some(Offers(vec![wheat_for_an_emerald()])),
    );
    let traded = notify_trade(&mut master, VILLAGER, 0, &mut random(), 0, None, 0, None);
    assert!(!traded.levelled);
}

/// The trader's trade: an orb of `3 + nextInt(4)` where the offer rewards experience,
/// none where it does not, and no experience of its own.
#[test]
fn a_trader_s_trade_drops_an_orb_only_where_the_offer_rewards_experience() {
    let mut mob = Mob::fresh();
    let mut offer = wheat_for_an_emerald();
    set_offers(&mut mob, TRADER, Some(Offers(vec![offer.clone()])));
    let mut drawn = random();
    let traded = notify_trade(&mut mob, TRADER, 0, &mut drawn, 0, Some(player(1)), 0, None);
    let mut replay = random();
    assert_eq!(traded.orb, Some(3 + replay.next_int(4) as i32));
    assert_eq!(drawn, replay);
    offer.reward_exp = false;
    set_offers(&mut mob, TRADER, Some(Offers(vec![offer])));
    let mut drawn = random();
    let traded = notify_trade(&mut mob, TRADER, 0, &mut drawn, 0, None, 0, None);
    assert_eq!(traded.orb, None);
    assert_eq!(drawn, random(), "nothing drawn");
}

/// `notifyTradeUpdated`: said only where the ambient sound has waited more than
/// `20 − interval`, then put off its interval; yes to a result, no to none.
#[test]
fn the_merchant_says_yes_or_no_only_where_its_ambient_sound_has_waited() {
    let mut mob = villager_with(professions::FARMER, 1, 0, None);
    mob.body.idle.ambient_since = 100 + 80;
    // At 100 the time is -80: not above -60.
    assert_eq!(
        notify_trade_updated(&mut mob, VILLAGER, 100, true, &mut random()),
        None
    );
    // At 121 it is -59.
    let mut drawn = random();
    let said = notify_trade_updated(&mut mob, VILLAGER, 121, true, &mut drawn);
    let mut replay = random();
    let pitch = (replay.next_float() - replay.next_float()) * 0.2 + 1.0;
    assert_eq!(said, Some((sounds::ENTITY_VILLAGER_YES, pitch)));
    assert_eq!(mob.body.idle.ambient_since, 121 + 80);
    mob.body.idle.ambient_since = 0;
    let said = notify_trade_updated(&mut mob, TRADER, 500, false, &mut random());
    assert_eq!(
        said.map(|(sound, _)| sound),
        Some(sounds::ENTITY_WANDERING_TRADER_NO)
    );
}

fn used(mob: &mut Mob, uses: i32) {
    villager::update(mob, |own| {
        if let Some(offers) = own.offers.as_mut() {
            for _ in 0..uses {
                offers.0[0].increase_uses();
            }
        }
    });
}

fn first_offer(mob: &Mob) -> Offer {
    villager::state_of(mob).1.offers.expect("offers").0[0].clone()
}

/// Restock twice a day: the first at once once an offer is used, the second only 2400
/// ticks after it, none more that day; a gap of more than 12000 ticks since the last
/// starts the day's count again. A restock updates demand, then sets the uses back.
#[test]
fn a_villager_restocks_twice_a_day_at_most_and_2400_ticks_apart() {
    let mut mob = villager_with(
        professions::FARMER,
        1,
        0,
        Some(Offers(vec![wheat_for_an_emerald()])),
    );
    // Nothing used: nothing to restock.
    assert!(!restock_if_due(&mut mob, 1000, 1000));
    used(&mut mob, 10);
    assert!(restock_if_due(&mut mob, 1000, 1000));
    let offer = first_offer(&mob);
    assert_eq!((offer.uses, offer.demand), (0, 10 - 6));
    let own = villager::state_of(&mob).1;
    assert_eq!((own.restocks_today, own.last_restock), (1, 1000));
    used(&mut mob, 16);
    assert!(!restock_if_due(&mut mob, 3400, 3400), "2400 not yet past");
    assert!(restock_if_due(&mut mob, 3401, 3401));
    assert_eq!(first_offer(&mob).demand, 4 + 16);
    used(&mut mob, 1);
    assert!(!restock_if_due(&mut mob, 9000, 9000), "two a day");
    // More than 12000 since the last: the count starts again, after two restocks no
    // demand is made up, and it restocks.
    assert!(!restock_if_due(&mut mob, 15_401, 15_401));
    assert!(restock_if_due(&mut mob, 15_402, 15_402));
    let own = villager::state_of(&mob).1;
    assert_eq!((own.restocks_today, own.last_restock), (1, 15_402));
    assert_eq!(first_offer(&mob).demand, 20 + 1 - 15);
}

/// A new day of the overworld's clock (where a day was checked before) starts the count
/// again, making up the demand of each restock missed: the uses set back, then the
/// demand updated once a restock missed.
#[test]
fn a_new_day_makes_up_the_demand_of_the_restocks_missed() {
    let mut mob = villager_with(
        professions::FARMER,
        1,
        0,
        Some(Offers(vec![wheat_for_an_emerald()])),
    );
    used(&mut mob, 4);
    // A check on day 1 at game time 5000.
    assert!(restock_if_due(&mut mob, 5000, 30_000));
    assert_eq!(villager::state_of(&mob).1.last_restock_check_day, 1);
    assert_eq!(first_offer(&mob).demand, 4 - 12);
    used(&mut mob, 5);
    // Day 2, within 12000 of the last restock: one restock missed is made up (uses back
    // to nought, demand −16), then it restocks nothing, as nothing is used any more.
    assert!(!restock_if_due(&mut mob, 10_000, 48_000));
    let offer = first_offer(&mob);
    assert_eq!((offer.uses, offer.demand), (0, -8 - 16));
    let own = villager::state_of(&mob).1;
    assert_eq!((own.restocks_today, own.last_restock), (0, 10_000));
}

/// A villager's window is titled by its profession.
#[test]
fn a_villager_s_window_is_titled_by_its_profession() {
    let mob = villager_with(professions::FARMER, 1, 0, None);
    assert_eq!(title(&mob, VILLAGER), "entity.minecraft.villager.farmer");
    let mob = villager_with(professions::NONE, 1, 0, None);
    assert_eq!(title(&mob, VILLAGER), "entity.minecraft.villager.none");
    assert_eq!(window_facts(&mob, VILLAGER), (1, 0, true, true));
}
