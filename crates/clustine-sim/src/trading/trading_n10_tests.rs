//! Drawing offers, from the specification (`docs/groundwork/n10-dumps.md`, part B,
//! sections 4, 5 and 8; the plan's section 8.1): each modifier alone, the number
//! providers, `getOffer`'s order and clamp, a set's filling, the librarian's book and the
//! cartographer's maps.

use clustine_data::enchanting;
use clustine_data::items;
use clustine_data::trading::{
    ContextIntProvider, EnchantmentPredicate, ItemFilter, ItemSet, ItemSubPredicate, NamedSet,
    StewEffect, TradeCost, TradeItem, TradeModifier, TradeSet, VillagerTrade, profession_trade_set,
    trade_id_by_name, villager_trade,
};
use clustine_world::components::{Component, Kind, PotionContents};
use clustine_world::{EntityId, ItemStack};

use super::evaluate::{TradeStack, apply, apply_all, int_value, mixed_colour, passes};
use super::offers::{MerchantContext, add_offers_from_trade_set, get_offer, to_item_cost};
use crate::items::enchanting::{enchantments_of, next_int_between};
use crate::region::Random;

const PLAINS: MerchantContext<'static> = MerchantContext {
    villager_type: Some("minecraft:plains"),
};

const EMERALD: u16 = items::EMERALD as u16;
const BOOK: u16 = items::BOOK as u16;
const ENCHANTED_BOOK: u16 = items::ENCHANTED_BOOK as u16;

fn random(seed: u64) -> Random {
    Random::of_entity_aside(0x5EED, EntityId(seed), 1, 2)
}

fn trade(name: &str) -> &'static VillagerTrade {
    villager_trade(trade_id_by_name(name).unwrap_or_else(|| panic!("no trade {name}")))
}

fn stored(stack: &ItemStack) -> Vec<(String, i32)> {
    match stack.components.get(Kind::StoredEnchantments) {
        Some(Component::StoredEnchantments(levels)) => levels.clone(),
        _ => Vec::new(),
    }
}

/// A trade of `wants` emeralds for `gives` through `modifiers`, as a file with nothing
/// else would read.
fn plain_trade(
    wants: i32,
    gives: u16,
    modifiers: &'static [TradeModifier],
    doubling: Option<NamedSet>,
) -> VillagerTrade {
    VillagerTrade {
        name: "test",
        wants: TradeCost {
            item: EMERALD,
            count: ContextIntProvider::Constant(wants),
            components: &[],
        },
        additional_wants: None,
        gives: TradeItem {
            item: gives,
            count: 1,
            components: &[],
        },
        max_uses: ContextIntProvider::Constant(4),
        xp: ContextIntProvider::Constant(1),
        reputation_discount: 0.2,
        given_item_modifier: modifiers,
        merchant_predicate: None,
        double_trade_price_enchantments: doubling,
    }
}

// The number providers.

#[test]
fn a_constant_draws_nothing_and_a_uniform_stays_within_its_bounds() {
    let mut drawn = random(1);
    let start = drawn.clone();
    assert_eq!(int_value(&ContextIntProvider::Constant(7), &mut drawn), 7);
    assert_eq!(
        int_value(&ContextIntProvider::Uniform { min: 3, max: 3 }, &mut drawn),
        3
    );
    assert_eq!(drawn.draws_since(&start), 0);
    let mut seen = [false; 15];
    for _ in 0..2000 {
        let value = int_value(&ContextIntProvider::Uniform { min: 5, max: 19 }, &mut drawn);
        assert!((5..=19).contains(&value));
        seen[(value - 5) as usize] = true;
    }
    assert!(
        seen.iter().all(|seen| *seen),
        "every value of 5 to 19 comes"
    );
}

/// `binomial`: `n` draws of a float, each counting below `p`; the leatherworker's `add`
/// of 1 and `binomial(2, 0.75)` is 1 to 3, 3 most often (0.5625), 1 least (0.0625).
#[test]
fn a_binomial_draws_n_floats_and_an_add_sums_its_inputs_in_order() {
    let binomial = ContextIntProvider::Binomial { n: 2, p: 0.75 };
    let mut drawn = random(2);
    let start = drawn.clone();
    int_value(&binomial, &mut drawn);
    assert_eq!(drawn.draws_since(&start), 2);
    const INPUTS: [ContextIntProvider; 2] = [
        ContextIntProvider::Constant(1),
        ContextIntProvider::Binomial { n: 2, p: 0.75 },
    ];
    let add = ContextIntProvider::Add(&INPUTS);
    let mut counts = [0u32; 4];
    for _ in 0..8000 {
        let value = int_value(&add, &mut drawn);
        assert!((1..=3).contains(&value));
        counts[value as usize] += 1;
    }
    assert!((300..700).contains(&counts[1]), "{counts:?}");
    assert!((4200..4800).contains(&counts[3]), "{counts:?}");
    // An overflowing sum is nought, as the game's caught `ArithmeticException` gives.
    const HUGE: [ContextIntProvider; 2] = [
        ContextIntProvider::Constant(i32::MAX),
        ContextIntProvider::Constant(1),
    ];
    assert_eq!(int_value(&ContextIntProvider::Add(&HUGE), &mut drawn), 0);
}

// Each modifier alone.

/// `enchant_randomly` on the librarian's enchanted book: one enchantment of `#tradeable`
/// stored at a level from 1 to its maximum, and the hidden cost `2 + nextInt(5 + 10·level)
/// + 3·level`; the draws are the pick, the level (none for a maximum of 1) and the cost.
#[test]
fn enchant_randomly_stores_one_tradeable_enchantment_and_its_hidden_cost() {
    const MODIFIER: TradeModifier = TradeModifier::EnchantRandomly {
        options: Some(NamedSet::Tag("#minecraft:tradeable")),
        only_compatible: false,
        include_additional_cost_component: true,
    };
    let tradeable = enchanting::enchantments_in(&NamedSet::Tag("#minecraft:tradeable"));
    for seed in 0..500 {
        let mut drawn = random(seed);
        let mut replay = drawn.clone();
        let mut book = TradeStack::of(ItemStack::new(items::ENCHANTED_BOOK, 1));
        apply(&MODIFIER, &mut book, &mut drawn);
        // The game's draws, replayed.
        let picked = tradeable[replay.next_int(tradeable.len() as u32) as usize];
        let level = next_int_between(&mut replay, 1, picked.max_level);
        let cost = 2 + replay.next_int((5 + 10 * level) as u32) as i32 + 3 * level;
        assert_eq!(drawn, replay, "the same draws, in the same order");
        let stack = book.stack.expect("an enchanted book");
        assert_eq!(stack.item, items::ENCHANTED_BOOK);
        assert_eq!(stored(&stack), [(picked.name.to_owned(), level)]);
        assert!(stack.components.get(Kind::Enchantments).is_none());
        assert_eq!(book.additional_trade_cost, Some(cost));
    }
    // Without the cost component nothing more is drawn.
    let mut drawn = random(3);
    let start = drawn.clone();
    let mut book = TradeStack::of(ItemStack::new(items::ENCHANTED_BOOK, 1));
    apply(
        &TradeModifier::EnchantRandomly {
            options: Some(NamedSet::Listed(&["minecraft:mending"])),
            only_compatible: false,
            include_additional_cost_component: false,
        },
        &mut book,
        &mut drawn,
    );
    // The pick of one, and no level for mending's maximum of 1.
    assert_eq!(drawn.draws_since(&start), 1);
    assert_eq!(book.additional_trade_cost, None);
}

/// `only_compatible` keeps the enchantments the item takes, but not for a book, which
/// becomes an enchanted book; where none is left the stack is as it was, with no draw.
#[test]
fn enchant_randomly_keeps_to_what_the_item_takes_and_a_book_becomes_enchanted() {
    const ANY_COMPATIBLE: TradeModifier = TradeModifier::EnchantRandomly {
        options: None,
        only_compatible: true,
        include_additional_cost_component: false,
    };
    for seed in 0..200 {
        let mut sword = TradeStack::of(ItemStack::new(items::DIAMOND_SWORD, 1));
        apply(&ANY_COMPATIBLE, &mut sword, &mut random(seed));
        let stack = sword.stack.expect("a sword");
        let levels = enchantments_of(&stack);
        assert_eq!(levels.len(), 1);
        let enchantment = enchanting::enchantment(&levels[0].0).expect("an enchantment");
        assert!(enchantment.is_supported_item(items::DIAMOND_SWORD));

        let mut book = TradeStack::of(ItemStack::new(items::BOOK, 3));
        apply(&ANY_COMPATIBLE, &mut book, &mut random(seed));
        let stack = book.stack.expect("an enchanted book");
        assert_eq!((stack.item, stack.count), (items::ENCHANTED_BOOK, 1));
        assert_eq!(stored(&stack).len(), 1);
    }
    let mut drawn = random(4);
    let start = drawn.clone();
    let mut stick = TradeStack::of(ItemStack::new(items::STICK, 1));
    apply(&ANY_COMPATIBLE, &mut stick, &mut drawn);
    assert_eq!(stick, TradeStack::of(ItemStack::new(items::STICK, 1)));
    assert_eq!(drawn.draws_since(&start), 0);
}

/// `enchant_with_levels` on the armourer's boots: the levels drawn first, then the
/// table's enchanting of `#on_traded_equipment`, the levels as the hidden cost.
#[test]
fn enchant_with_levels_draws_its_levels_first_and_keeps_them_as_the_cost() {
    let boots = trade("minecraft:armorer/4/emerald_enchanted_diamond_boots");
    let modifier = &boots.given_item_modifier[0];
    let on_traded = NamedSet::Tag("#minecraft:on_traded_equipment");
    for seed in 0..300 {
        let mut drawn = random(seed);
        let mut replay = drawn.clone();
        let mut stack = TradeStack::of(ItemStack::new(items::DIAMOND_BOOTS, 1));
        apply(modifier, &mut stack, &mut drawn);
        let levels = next_int_between(&mut replay, 5, 19);
        assert_eq!(stack.additional_trade_cost, Some(levels));
        let boots = stack.stack.expect("boots");
        assert_eq!(boots.item, items::DIAMOND_BOOTS);
        for (name, _) in enchantments_of(&boots) {
            assert!(enchanting::enchantment_in(&on_traded, name), "{name}");
            let enchantment = enchanting::enchantment(name).expect("an enchantment");
            assert!(enchantment.is_primary_item(items::DIAMOND_BOOTS), "{name}");
        }
        assert!(stored(&boots).is_empty());
    }
}

/// `filtered`: `on_pass` where the item passes, `on_fail` where it does not, nothing
/// where the branch has none.
#[test]
fn filtered_applies_the_branch_the_item_takes() {
    const SWORDS_WITH_ENCHANTMENTS: ItemFilter = ItemFilter {
        items: Some(ItemSet::Tag("#minecraft:swords")),
        predicates: &[ItemSubPredicate::Enchantments(&[EnchantmentPredicate {
            enchantments: None,
            min_level: None,
            max_level: None,
        }])],
    };
    const FILTER: TradeModifier = TradeModifier::Filtered {
        item_filter: SWORDS_WITH_ENCHANTMENTS,
        on_pass: &[TradeModifier::SetCount {
            count: ContextIntProvider::Constant(2),
            add: true,
        }],
        on_fail: &[TradeModifier::Discard],
    };
    let mut sword = ItemStack::new(items::IRON_SWORD, 1);
    assert!(!passes(&SWORDS_WITH_ENCHANTMENTS, &sword));
    let mut plain = TradeStack::of(sword.clone());
    apply(&FILTER, &mut plain, &mut random(5));
    assert_eq!(plain.stack, None);
    sword.components.set(Component::Enchantments(vec![(
        "minecraft:sharpness".into(),
        1,
    )]));
    let mut enchanted = TradeStack::of(sword.clone());
    apply(&FILTER, &mut enchanted, &mut random(5));
    assert_eq!(enchanted.stack, Some(sword.with_count(3)));
    // An axe is no sword, whatever it has.
    let mut axe = ItemStack::new(items::IRON_AXE, 1);
    axe.components.set(Component::Enchantments(vec![(
        "minecraft:sharpness".into(),
        1,
    )]));
    assert!(!passes(&SWORDS_WITH_ENCHANTMENTS, &axe));
    // Level bounds and a set.
    let bounded = ItemFilter {
        items: None,
        predicates: &[ItemSubPredicate::Enchantments(&[EnchantmentPredicate {
            enchantments: Some(NamedSet::Listed(&["minecraft:sharpness"])),
            min_level: Some(2),
            max_level: None,
        }])],
    };
    assert!(!passes(&bounded, &sword));
    sword.components.set(Component::Enchantments(vec![(
        "minecraft:sharpness".into(),
        2,
    )]));
    assert!(passes(&bounded, &sword));
}

/// `set_random_dyes` on the leatherworker's leggings: 1 to 3 dyes, each one of the 16
/// colours drawn, mixed into a colour; the count becomes 1.
#[test]
fn set_random_dyes_mixes_the_dyes_drawn_into_one_colour() {
    let leggings = trade("minecraft:leatherworker/1/emerald_dyed_leather_leggings");
    for seed in 0..300 {
        let mut drawn = random(seed);
        let start = drawn.clone();
        let mut stack = TradeStack::of(ItemStack::new(items::LEATHER_LEGGINGS, 1));
        apply_all(leggings.given_item_modifier, &mut stack, &mut drawn);
        let stack = stack.stack.expect("dyed leggings pass the filter");
        assert!(matches!(
            stack.components.get(Kind::DyedColor),
            Some(Component::DyedColor(rgb)) if (0..=0x00FF_FFFF).contains(rgb)
        ));
        // Two floats for the binomial, then one draw a dye, one to three dyes.
        let draws = drawn.draws_since(&start);
        assert!((3..=5).contains(&draws), "{draws}");
    }
}

/// `DyedItemColor.applyDyes` worked by hand: red alone is red's own colour; red and blue
/// average to (118, 57, 104), the average brightest channel is (176 + 170) / 2 = 173, so
/// each channel is scaled by 173 / 118 and truncated: (173, 83, 152).
#[test]
fn dyes_mix_as_the_game_s_arithmetic_has_it() {
    let red = 11_546_150; // 0xB02E26
    let blue = 3_949_738; // 0x3C44AA
    assert_eq!(mixed_colour(None, &[red]), 0x00B0_2E26);
    assert_eq!(mixed_colour(None, &[red, blue]), 0x00AD_5398);
    // An old colour counts as one more dye.
    assert_eq!(mixed_colour(Some(0x00B0_2E26), &[blue]), 0x00AD_5398);
}

/// `set_random_potion` on the fletcher's tipped arrows: a potion of the potions'
/// `#tradeable`, drawn once.
#[test]
fn set_random_potion_gives_a_tradeable_potion() {
    let arrows = trade("minecraft:fletcher/5/arrow_and_emerald_tipped_arrow");
    let names: Vec<&str> = clustine_data::potions::POTIONS
        .iter()
        .map(|p| p.name)
        .collect();
    let tradeable = enchanting::tag_names("minecraft:potion", "#minecraft:tradeable", &names);
    assert_eq!(tradeable.len(), 41);
    assert_eq!(tradeable[0], "minecraft:wind_charged");
    for seed in 0..100 {
        let mut drawn = random(seed);
        let start = drawn.clone();
        let mut stack = TradeStack::of(ItemStack::new(items::TIPPED_ARROW, 5));
        apply_all(arrows.given_item_modifier, &mut stack, &mut drawn);
        assert_eq!(drawn.draws_since(&start), 1);
        let stack = stack.stack.expect("arrows");
        assert_eq!(stack.count, 5);
        match stack.components.get(Kind::PotionContents) {
            Some(Component::PotionContents(PotionContents {
                potion: Some(potion),
                ..
            })) => assert!(tradeable.contains(&potion.as_str()), "{potion}"),
            other => panic!("no potion: {other:?}"),
        }
    }
}

/// `set_stew_effect` on the farmer's stew: one of its six effects, its seconds made ticks
/// but for saturation, which is instantaneous; on another item, nothing.
#[test]
fn set_stew_effect_adds_one_effect_its_seconds_in_ticks() {
    let stew = trade("minecraft:farmer/4/emerald_suspicious_stew");
    let mut seen = Vec::new();
    for seed in 0..200 {
        let mut stack = TradeStack::of(ItemStack::new(items::SUSPICIOUS_STEW, 1));
        apply_all(stew.given_item_modifier, &mut stack, &mut random(seed));
        let Some(Component::SuspiciousStewEffects(effects)) = stack
            .stack
            .expect("a stew")
            .components
            .get(Kind::SuspiciousStewEffects)
            .cloned()
        else {
            panic!("no effects");
        };
        assert_eq!(effects.len(), 1);
        let (effect, duration) = &effects[0];
        let wanted = match effect.as_str() {
            "minecraft:night_vision" => 100,
            "minecraft:jump_boost" => 160,
            "minecraft:weakness" => 140,
            "minecraft:blindness" => 120,
            "minecraft:poison" => 280,
            "minecraft:saturation" => 7,
            other => panic!("{other}"),
        };
        assert_eq!(*duration, wanted);
        if !seen.contains(effect) {
            seen.push(effect.clone());
        }
    }
    assert_eq!(seen.len(), 6);
    const ONE: TradeModifier = TradeModifier::SetStewEffect {
        effects: &[StewEffect {
            effect: "minecraft:poison",
            duration: ContextIntProvider::Constant(1),
        }],
    };
    let mut bread = TradeStack::of(ItemStack::new(items::BREAD, 1));
    let mut drawn = random(6);
    let start = drawn.clone();
    apply(&ONE, &mut bread, &mut drawn);
    assert_eq!(bread, TradeStack::of(ItemStack::new(items::BREAD, 1)));
    assert_eq!(drawn.draws_since(&start), 0);
}

#[test]
fn set_potion_set_count_and_discard_do_what_they_say() {
    let invisibility = trade("minecraft:wandering_trader/emerald_long_invisibility_potion");
    let mut potion = TradeStack::of(ItemStack::new(items::POTION, 1));
    apply_all(
        invisibility.given_item_modifier,
        &mut potion,
        &mut random(7),
    );
    assert_eq!(
        potion
            .stack
            .expect("a potion")
            .components
            .get(Kind::PotionContents),
        Some(&Component::PotionContents(PotionContents {
            potion: Some("minecraft:long_invisibility".into()),
            ..PotionContents::default()
        }))
    );
    let mut stack = TradeStack::of(ItemStack::new(items::ARROW, 4));
    apply(
        &TradeModifier::SetCount {
            count: ContextIntProvider::Constant(9),
            add: false,
        },
        &mut stack,
        &mut random(7),
    );
    assert_eq!(stack.stack.as_ref().map(|s| s.count), Some(9));
    apply(
        &TradeModifier::SetCount {
            count: ContextIntProvider::Constant(-9),
            add: true,
        },
        &mut stack,
        &mut random(7),
    );
    assert_eq!(stack.stack, None);
    let mut kept = TradeStack::of(ItemStack::new(items::ARROW, 4));
    apply(&TradeModifier::Discard, &mut kept, &mut random(7));
    assert_eq!(kept.stack, None);
}

/// `exploration_map` finds nothing (decision 9) and leaves the map as it was, which the
/// trade's `filtered` on `map_id` then discards, as the game does where no structure is
/// found; nothing is drawn.
#[test]
fn an_explorer_map_is_never_made_and_its_trade_gives_no_offer() {
    let map = trade("minecraft:cartographer/2/emerald_and_compass_explorer_jungle_map");
    assert!(matches!(
        map.given_item_modifier[0],
        TradeModifier::ExplorationMap { .. }
    ));
    let mut drawn = random(8);
    let start = drawn.clone();
    let mut stack = TradeStack::of(ItemStack::new(i32::from(map.gives.item), 1));
    apply(&map.given_item_modifier[0], &mut stack, &mut drawn);
    assert_eq!(
        stack.stack.as_ref().map(|s| s.item),
        Some(i32::from(map.gives.item))
    );
    assert_eq!(get_offer(map, &PLAINS, &mut drawn), None);
    assert_eq!(drawn.draws_since(&start), 0);
}

// `getOffer`.

/// The wanted cost is its count plus the hidden cost, kept within 0 and the item's
/// stack: the 30 levels of a book's enchanting added to 50 emeralds come to 64.
#[test]
fn the_hidden_cost_is_added_to_the_wanted_count_and_clamped_to_a_stack() {
    const MODIFIERS: [TradeModifier; 1] = [TradeModifier::EnchantWithLevels {
        levels: ContextIntProvider::Constant(30),
        options: None,
        include_additional_cost_component: true,
    }];
    let book = plain_trade(50, BOOK, &MODIFIERS, None);
    let offer = get_offer(&book, &PLAINS, &mut random(9)).expect("an offer");
    assert_eq!(offer.cost_a.count, 64);
    assert_eq!(offer.result.item, items::ENCHANTED_BOOK);
    let cheap = plain_trade(10, BOOK, &MODIFIERS, None);
    let offer = get_offer(&cheap, &PLAINS, &mut random(9)).expect("an offer");
    assert_eq!(offer.cost_a.count, 40);
    // The constructor trades use: no uses, rewarded, no special price or demand.
    assert_eq!(
        (offer.uses, offer.max_uses, offer.xp, offer.reward_exp),
        (0, 4, 1, true)
    );
    assert_eq!((offer.special_price, offer.demand), (0, 0));
    assert_eq!(offer.price_multiplier.0, 0.2);
    // The clamp in `toItemCost` is to 0, not 1, and `getOffer` gives none below 1.
    let free = plain_trade(0, items::STICK as u16, &[], None);
    assert_eq!(get_offer(&free, &PLAINS, &mut random(9)), None);
    let mut drawn = random(9);
    let cost = to_item_cost(&free.wants, -5, &mut drawn);
    assert_eq!(cost.count, 0);
}

/// Only the hidden cost is doubled, and only where the result stores an enchantment of
/// the doubling set: mending on a book doubles, mending on a sword's own enchantments
/// does not, and sharpness on a book does not.
#[test]
fn the_hidden_cost_doubles_only_for_a_stored_enchantment_of_the_set() {
    const MENDING: [TradeModifier; 1] = [TradeModifier::EnchantRandomly {
        options: Some(NamedSet::Listed(&["minecraft:mending"])),
        only_compatible: false,
        include_additional_cost_component: true,
    }];
    const SHARPNESS: [TradeModifier; 1] = [TradeModifier::EnchantRandomly {
        options: Some(NamedSet::Listed(&["minecraft:sharpness"])),
        only_compatible: false,
        include_additional_cost_component: true,
    }];
    let doubling = Some(NamedSet::Tag("#minecraft:double_trade_price"));
    for seed in 0..100 {
        // Mending's level is 1: the hidden cost is 2 + nextInt(15) + 3.
        let replay = |seed| {
            let mut replay = random(seed);
            replay.next_int(1);
            5 + replay.next_int(15) as i32
        };
        let extra = replay(seed);
        let book = plain_trade(3, ENCHANTED_BOOK, &MENDING, doubling);
        let offer = get_offer(&book, &PLAINS, &mut random(seed)).expect("a book");
        assert_eq!(stored(&offer.result), [("minecraft:mending".to_owned(), 1)]);
        assert_eq!(offer.cost_a.count, 3 + 2 * extra);
        let sword = plain_trade(3, items::DIAMOND_SWORD as u16, &MENDING, doubling);
        let offer = get_offer(&sword, &PLAINS, &mut random(seed)).expect("a sword");
        assert_eq!(offer.cost_a.count, 3 + extra);
        let unlisted = plain_trade(3, ENCHANTED_BOOK, &SHARPNESS, doubling);
        let offer = get_offer(&unlisted, &PLAINS, &mut random(seed)).expect("a book");
        let level = stored(&offer.result)[0].1;
        assert!(offer.cost_a.count <= 3 + 6 + 13 * level);
        assert!(offer.cost_a.count >= 3 + 2 + 3 * level);
        let undoubled = plain_trade(3, ENCHANTED_BOOK, &MENDING, None);
        let offer = get_offer(&undoubled, &PLAINS, &mut random(seed)).expect("a book");
        assert_eq!(offer.cost_a.count, 3 + extra);
    }
}

/// A trade with a merchant predicate is made only by a villager of a type listed, and
/// never by the wandering trader, who has none.
#[test]
fn a_trade_for_some_villager_types_is_made_only_by_them() {
    let taiga_map = trade("minecraft:cartographer/2/emerald_and_compass_village_taiga_map");
    let predicate = taiga_map.merchant_predicate.expect("a predicate");
    assert!(predicate.villager_variants.contains(&"minecraft:plains"));
    let glass = trade("minecraft:cartographer/2/glass_pane_emerald");
    let trader = MerchantContext {
        villager_type: None,
    };
    assert!(get_offer(glass, &trader, &mut random(10)).is_some());
    // The predicate is tested before any draw: a desert villager draws nothing.
    let desert = MerchantContext {
        villager_type: Some("minecraft:desert"),
    };
    let mut drawn = random(10);
    let start = drawn.clone();
    assert_eq!(get_offer(taiga_map, &desert, &mut drawn), None);
    assert_eq!(get_offer(taiga_map, &trader, &mut drawn), None);
    assert_eq!(drawn.draws_since(&start), 0);
}

/// The wandering trader's water bottle is wanted by its potion, the exact predicate a
/// part of the cost.
#[test]
fn a_wanted_water_bottle_names_its_potion() {
    let water = clustine_data::trading::villager_trades()
        .iter()
        .find(|trade| !trade.wants.components.is_empty())
        .expect("the water bottle trade");
    let offer = get_offer(
        water,
        &MerchantContext {
            villager_type: None,
        },
        &mut random(11),
    )
    .expect("an offer");
    let mut bottle = ItemStack::new(items::POTION, 1);
    assert!(!offer.cost_a.test(&bottle));
    bottle
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:water".into()),
            ..PotionContents::default()
        }));
    assert!(offer.cost_a.test(&bottle));
    assert_eq!(offer.cost_a().components, bottle.components);
}

// `addOffersFromTradeSet`.

/// Without duplicates a set gives exactly its amount, each from another trade, where
/// enough trades succeed: the librarian's level 1 gives 2 of its 3.
#[test]
fn a_set_gives_its_amount_of_offers_from_different_trades() {
    let set = profession_trade_set("minecraft:librarian", 1).expect("a set");
    for seed in 0..300 {
        let offers = add_offers_from_trade_set(set, &PLAINS, &mut random(seed));
        assert_eq!(offers.len(), 2);
        assert_ne!(offers[0].result.item, offers[1].result.item);
    }
}

/// A failed trade is not counted: the cartographer's level 2 has seven maps, which
/// always fail, and a glass pane trade, so it gives that one offer and never a map,
/// however many picks it takes.
#[test]
fn a_cartographer_s_second_level_never_offers_a_map() {
    let set = profession_trade_set("minecraft:cartographer", 2).expect("a set");
    assert_eq!(set.trades.len(), 8);
    for seed in 0..300 {
        for villager_type in ["minecraft:plains", "minecraft:desert", "minecraft:taiga"] {
            let merchant = MerchantContext {
                villager_type: Some(villager_type),
            };
            let offers = add_offers_from_trade_set(set, &merchant, &mut random(seed));
            assert_eq!(offers.len(), 1, "only the glass pane succeeds");
            assert_eq!(offers[0].result.item, items::EMERALD);
            assert_eq!(offers[0].cost_a.item, items::GLASS_PANE);
        }
    }
}

/// With duplicates a successful trade stays to be picked again, a failed one is
/// removed: a set of one trade and one map gives its amount of the one trade.
#[test]
fn with_duplicates_a_trade_that_succeeds_may_come_again() {
    let paper = trade_id_by_name("minecraft:librarian/1/paper_emerald").expect("paper");
    let map = trade_id_by_name("minecraft:cartographer/2/emerald_and_compass_explorer_jungle_map")
        .expect("a map");
    let trades: &'static [u16] = Box::leak(vec![paper, map].into_boxed_slice());
    let set = TradeSet {
        name: "test",
        amount: ContextIntProvider::Constant(3),
        allow_duplicates: true,
        random_sequence: None,
        trades,
    };
    for seed in 0..100 {
        let offers = add_offers_from_trade_set(&set, &PLAINS, &mut random(seed));
        assert_eq!(offers.len(), 3);
        assert!(offers.iter().all(|offer| offer.cost_a.item == items::PAPER));
    }
    let without = TradeSet {
        allow_duplicates: false,
        ..set
    };
    assert_eq!(
        add_offers_from_trade_set(&without, &PLAINS, &mut random(1)).len(),
        1
    );
}

/// The librarian's level-1 book over many draws: one stored enchantment of
/// `#tradeable` at a level within its range, priced at `2 + nextInt(5 + 10·level) +
/// 3·level` emeralds, doubled for the treasure ones of `#double_trade_price`, at most a
/// stack, with a book as the second cost; every tradeable enchantment comes in time.
#[test]
fn the_librarian_s_first_book_is_one_tradeable_enchantment_at_a_fair_price() {
    let book_trade = trade("minecraft:librarian/1/emerald_and_book_enchanted_book");
    let tradeable = NamedSet::Tag("#minecraft:tradeable");
    let doubling = NamedSet::Tag("#minecraft:double_trade_price");
    let mut seen: Vec<String> = Vec::new();
    let mut doubled = 0;
    for seed in 0..4000 {
        let offer = get_offer(book_trade, &PLAINS, &mut random(seed)).expect("a book always");
        assert_eq!(offer.result.item, items::ENCHANTED_BOOK);
        let levels = stored(&offer.result);
        assert_eq!(levels.len(), 1);
        let (name, level) = &levels[0];
        assert!(enchanting::enchantment_in(&tradeable, name), "{name}");
        let enchantment = enchanting::enchantment(name).expect("an enchantment");
        assert!((1..=enchantment.max_level).contains(level));
        let (low, high) = (2 + 3 * level, 6 + 13 * level);
        let (low, high) = if enchanting::enchantment_in(&doubling, name) {
            doubled += 1;
            (2 * low, 2 * high)
        } else {
            (low, high)
        };
        assert!(
            (low.min(64)..=high.min(64)).contains(&offer.cost_a.count),
            "{name} {level} at {}",
            offer.cost_a.count
        );
        assert_eq!(offer.cost_a.item, items::EMERALD);
        let book = offer.cost_b.as_ref().expect("a book wanted");
        assert_eq!((book.item, book.count), (items::BOOK, 1));
        assert_eq!((offer.max_uses, offer.xp), (12, 1));
        assert_eq!(offer.price_multiplier.0, 0.2);
        if !seen.contains(name) {
            seen.push(name.clone());
        }
    }
    let all = enchanting::enchantments_in(&tradeable).len();
    assert_eq!(seen.len(), all, "every tradeable enchantment comes");
    // Four of the tradeable ones double (mending, frost walker and the two curses).
    assert!(doubled > 4000 * 4 / all / 2, "{doubled}");
}
