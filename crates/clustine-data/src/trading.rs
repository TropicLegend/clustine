//! The villagers' and the wandering trader's trades, compiled by `cargo datagen` from the
//! game's `villager_trade/**`, `trade_set/**` and `tags/villager_trade/**`
//! (`docs/groundwork/n10-brief.md`, section 5.2; `docs/groundwork/n10-dumps.md`, part B,
//! sections 5 and 8, and what the lead settled afterwards).
//!
//! A trade is as its file has it, every default filled in as the codecs fill it
//! (`VillagerTrade`'s: `max_uses` 4, `xp` 1, `reputation_discount` 0; `TradeCost`'s: a
//! count of 1; `ItemStackTemplate`'s: a count of 1). Nothing here is evaluated: drawing
//! an offer from a trade (`VillagerTrade.getOffer`) is the evaluator's. A trade set names
//! its trades by their numbers among [`villager_trades`], its tag expanded in place in
//! the tag's order, nested tags where they stand, each trade once.
//!
//! Items are named by their ids in [`crate::ITEMS`]; enchantments, potions, mob effects,
//! villager types, map decorations and structures by their names, and tags by theirs
//! with `#`. The trades' item modifiers are a sibling of [`crate::loot::LootModifier`]
//! rather than more of its variants: those are matched exhaustively by the loot
//! evaluators, which never meet a trade's, and a trade's `filtered` nests modifiers where
//! a loot table's name them by number. Only what the game's trades use is here; `cargo
//! datagen` refuses a file with anything else.

use crate::generated::trades::{TRADE_SETS, VILLAGER_TRADES};

/// A trade's number among [`villager_trades`].
pub type TradeId = u16;

/// A whole number as the game's `ContextIntProvider`s give it. The loot tables' own
/// [`crate::loot::IntProvider`] has no `add`, and its evaluators match it exhaustively.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextIntProvider {
    Constant(i32),
    /// `Mth.nextInt(random, min, max)`: one draw unless `min >= max`.
    Uniform {
        min: i32,
        max: i32,
    },
    /// `n` draws, each counting where it is below `p`.
    Binomial {
        n: i32,
        p: f32,
    },
    /// The sum of the inputs, drawn in order (`minecraft:add`).
    Add(&'static [ContextIntProvider]),
}

/// A set of entries of a registry (the game's `HolderSet`): a tag, by its name with `#`,
/// or the entries listed by name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NamedSet {
    Tag(&'static str),
    Listed(&'static [&'static str]),
}

/// A set of items: a tag, by its name with `#`, or the items listed by id.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemSet {
    Tag(&'static str),
    Listed(&'static [u16]),
}

/// A component value a trade's item has to have (`DataComponentExactPredicate`) or is
/// given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TradeComponent {
    /// `minecraft:potion_contents` with this potion and nothing else: the wandering
    /// trader's water bottle.
    PotionContents { potion: &'static str },
}

/// What a trade wants (`TradeCost`). The cost of an offer is `clamp(count + extra, 0,
/// the item's max stack size)`, where `extra` is what the modifiers added (only for
/// `wants`, not `additional_wants`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradeCost {
    pub item: u16,
    /// 1 where the file has none.
    pub count: ContextIntProvider,
    /// Exactly these values, which the paid item has to have.
    pub components: &'static [TradeComponent],
}

/// What a trade gives before its modifiers (`ItemStackTemplate`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradeItem {
    pub item: u16,
    /// 1 to 99; 1 where the file has none.
    pub count: i32,
    pub components: &'static [TradeComponent],
}

/// Who may offer a trade: `entity_properties` on the villager itself (`this`) whose
/// variant, its villager type, is one of these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MerchantPredicate {
    pub villager_variants: &'static [&'static str],
}

/// One enchantment that an item has to have (`EnchantmentPredicate`): of the set where
/// there is one, else any; at a level within the bounds that are there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnchantmentPredicate {
    pub enchantments: Option<NamedSet>,
    pub min_level: Option<i32>,
    pub max_level: Option<i32>,
}

/// A part of an item predicate's `predicates` (the game's `DataComponentPredicate`s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemSubPredicate {
    /// `minecraft:enchantments`: each of these holds of the item's enchantments.
    Enchantments(&'static [EnchantmentPredicate]),
    /// `minecraft:stored_enchantments`: each of these holds of the item's stored ones.
    StoredEnchantments(&'static [EnchantmentPredicate]),
    /// `minecraft:dyed_color` with nothing in it: the item has a colour.
    DyedColor,
    /// `minecraft:map_id` with nothing in it: the item has a map id.
    MapId,
}

/// The item predicate of a `filtered`: the item is of the set where there is one, and
/// every part holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemFilter {
    pub items: Option<ItemSet>,
    pub predicates: &'static [ItemSubPredicate],
}

/// A suspicious stew's effect that `set_stew_effect` may choose: the effect and its
/// duration in seconds, which the game multiplies by 20 for an effect that is not
/// instantaneous.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StewEffect {
    pub effect: &'static str,
    pub duration: ContextIntProvider,
}

/// An item modifier of a trade (the game's `LootItemFunction`), applied to what it gives
/// in order. None of the game's trades has a condition on one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TradeModifier {
    /// `enchant_randomly`: one enchantment of `options` (every one where there is none),
    /// of those the item takes where `only_compatible`.
    EnchantRandomly {
        options: Option<NamedSet>,
        /// True where the file has none.
        only_compatible: bool,
        /// Whether the hidden `additional_trade_cost` is written; false where the file
        /// has none.
        include_additional_cost_component: bool,
    },
    /// `enchant_with_levels`: enchanted as an enchanting table would at the level drawn.
    EnchantWithLevels {
        levels: ContextIntProvider,
        options: Option<NamedSet>,
        include_additional_cost_component: bool,
    },
    /// `filtered`: `on_pass` where the item passes, else `on_fail` (no modifier where
    /// the file has none).
    Filtered {
        item_filter: ItemFilter,
        on_pass: &'static [TradeModifier],
        on_fail: &'static [TradeModifier],
    },
    /// `discard`: the item becomes empty, and the trade gives no offer.
    Discard,
    /// `set_random_dyes`: a leather item dyed with this many random dyes.
    SetRandomDyes { number_of_dyes: ContextIntProvider },
    /// `set_random_potion`: a potion of `options` (any where there is none).
    SetRandomPotion { options: Option<NamedSet> },
    /// `set_stew_effect`: one of the effects, chosen at random.
    SetStewEffect { effects: &'static [StewEffect] },
    /// `set_potion`: this potion.
    SetPotion { potion: &'static str },
    /// `set_count`: the count set, or added to.
    SetCount {
        count: ContextIntProvider,
        add: bool,
    },
    /// `exploration_map`: a map to the nearest structure of `destination`. Clustine
    /// keeps no structure references over the radius the search needs, so its
    /// evaluator treats every search as failed (the plan's decision 9), as the game
    /// does where none is found; the fields are kept all the same.
    ExplorationMap {
        destination: NamedSet,
        /// `minecraft:mansion` where the file has none.
        decoration: &'static str,
        /// 2 where the file has none.
        zoom: i8,
        /// 50 where the file has none.
        search_radius: i32,
        /// True where the file has none.
        skip_existing_chunks: bool,
    },
}

/// A trade (`VillagerTrade`), by its name such as `minecraft:librarian/1/paper_emerald`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VillagerTrade {
    pub name: &'static str,
    pub wants: TradeCost,
    pub additional_wants: Option<TradeCost>,
    pub gives: TradeItem,
    /// 4 where the file has none.
    pub max_uses: ContextIntProvider,
    /// 1 where the file has none.
    pub xp: ContextIntProvider,
    /// The price multiplier; 0 where the file has none.
    pub reputation_discount: f32,
    /// Applied in order to what the trade gives.
    pub given_item_modifier: &'static [TradeModifier],
    pub merchant_predicate: Option<MerchantPredicate>,
    /// Where a stored enchantment of the result is in this set, the cost the modifiers
    /// added is doubled.
    pub double_trade_price_enchantments: Option<NamedSet>,
}

/// A trade set (`TradeSet`), by its name such as `minecraft:armorer/level_1`: `amount`
/// trades drawn from `trades` with `random_sequence`'s random.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradeSet {
    pub name: &'static str,
    pub amount: ContextIntProvider,
    /// False where the file has none.
    pub allow_duplicates: bool,
    pub random_sequence: Option<&'static str>,
    pub trades: &'static [TradeId],
}

/// The names of the wandering trader's three sets.
pub const WANDERING_TRADER_BUYING: &str = "minecraft:wandering_trader/buying";
pub const WANDERING_TRADER_UNCOMMON: &str = "minecraft:wandering_trader/uncommon";
pub const WANDERING_TRADER_COMMON: &str = "minecraft:wandering_trader/common";

/// Every trade, sorted by name; a trade's [`TradeId`] is its index.
pub fn villager_trades() -> &'static [VillagerTrade] {
    &VILLAGER_TRADES
}

/// The trade of this number. Panics if there is none.
pub fn villager_trade(id: TradeId) -> &'static VillagerTrade {
    &VILLAGER_TRADES[usize::from(id)]
}

/// The number of the trade of this name.
pub fn trade_id_by_name(name: &str) -> Option<TradeId> {
    let index = VILLAGER_TRADES
        .binary_search_by(|trade| trade.name.cmp(name))
        .ok()?;
    Some(TradeId::try_from(index).expect("fewer than 65536 trades"))
}

/// Every trade set, sorted by name.
pub fn trade_sets() -> &'static [TradeSet] {
    &TRADE_SETS
}

/// The trade set of this name, such as [`WANDERING_TRADER_COMMON`].
pub fn trade_set_by_name(name: &str) -> Option<&'static TradeSet> {
    let index = TRADE_SETS.binary_search_by(|set| set.name.cmp(name)).ok()?;
    Some(&TRADE_SETS[index])
}

/// The set a villager of the profession of this registry name (`minecraft:armorer`)
/// draws from on reaching `level` (1 to 5; `VillagerProfession.getTrades`). `None` for
/// a profession without trades (`minecraft:none`, `minecraft:nitwit`) or another level.
pub fn profession_trade_set(profession: &str, level: i32) -> Option<&'static TradeSet> {
    let profession = profession.strip_prefix("minecraft:")?;
    trade_set_by_name(&format!("minecraft:{profession}/level_{level}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items;

    const PROFESSIONS: [(&str, [usize; 5]); 13] = [
        ("armorer", [5, 4, 5, 2, 2]),
        ("butcher", [4, 3, 2, 1, 1]),
        ("cartographer", [2, 8, 3, 16, 2]),
        ("cleric", [2, 2, 2, 3, 2]),
        ("farmer", [5, 3, 2, 2, 2]),
        ("fisherman", [4, 3, 2, 1, 6]),
        ("fletcher", [3, 2, 2, 2, 3]),
        ("leatherworker", [3, 3, 2, 2, 2]),
        ("librarian", [3, 3, 3, 4, 2]),
        ("mason", [2, 2, 7, 33, 2]),
        ("shepherd", [5, 37, 21, 22, 1]),
        ("toolsmith", [5, 2, 5, 3, 1]),
        ("weaponsmith", [3, 2, 1, 2, 1]),
    ];

    /// The trade sets of the brief's section 5.2: each profession's five, with as many
    /// trades as its level's tag has after the nested tags are expanded; two drawn from
    /// each but three at the librarian's level 5 and five of the trader's common.
    #[test]
    fn each_profession_has_five_sets_of_the_sizes_its_tags_give() {
        assert_eq!(villager_trades().len(), 391);
        assert_eq!(trade_sets().len(), 68);
        for (profession, sizes) in PROFESSIONS {
            for (level, size) in (1..=5).zip(sizes) {
                let set = profession_trade_set(&format!("minecraft:{profession}"), level)
                    .unwrap_or_else(|| panic!("{profession} has no set at level {level}"));
                assert_eq!(set.trades.len(), size, "{}", set.name);
                let amount = if (profession, level) == ("librarian", 5) {
                    3
                } else {
                    2
                };
                assert_eq!(
                    set.amount,
                    ContextIntProvider::Constant(amount),
                    "{}",
                    set.name
                );
                assert!(!set.allow_duplicates);
                assert_eq!(
                    set.random_sequence,
                    Some(format!("minecraft:trade_set/{profession}/level_{level}").as_str())
                );
                for &id in set.trades {
                    assert!(
                        villager_trade(id)
                            .name
                            .starts_with(&format!("minecraft:{profession}/"))
                            || villager_trade(id).name.starts_with("minecraft:smith/"),
                        "{} in {}",
                        villager_trade(id).name,
                        set.name
                    );
                }
            }
            assert!(profession_trade_set(&format!("minecraft:{profession}"), 6).is_none());
        }
        assert!(profession_trade_set("minecraft:none", 1).is_none());
        assert!(profession_trade_set("minecraft:nitwit", 1).is_none());
        for (name, amount) in [
            (WANDERING_TRADER_BUYING, 2),
            (WANDERING_TRADER_UNCOMMON, 2),
            (WANDERING_TRADER_COMMON, 5),
        ] {
            let set = trade_set_by_name(name).unwrap();
            assert_eq!(set.amount, ContextIntProvider::Constant(amount), "{name}");
        }
        assert!(villager_trades().is_sorted_by(|a, b| a.name < b.name));
        assert!(trade_sets().is_sorted_by(|a, b| a.name < b.name));
    }

    /// An armorer's level 1 is the common smith's level 1 first, then its own four.
    #[test]
    fn a_nested_tag_is_expanded_where_it_stands() {
        let set = profession_trade_set("minecraft:armorer", 1).unwrap();
        let names: Vec<&str> = set
            .trades
            .iter()
            .map(|&id| villager_trade(id).name)
            .collect();
        assert!(names[0].starts_with("minecraft:smith/1/"), "{names:?}");
        assert_eq!(
            names[1..],
            [
                "minecraft:armorer/1/emerald_iron_leggings",
                "minecraft:armorer/1/emerald_iron_boots",
                "minecraft:armorer/1/emerald_iron_helmet",
                "minecraft:armorer/1/emerald_iron_chestplate",
            ]
        );
    }

    /// `villager_trade/librarian/1/emerald_and_book_enchanted_book.json`, read back whole:
    /// a count of 0 written, a book of the default count, `xp` left to its default of 1,
    /// a random enchantment of `#tradeable` filtered to a book that has one.
    #[test]
    fn the_librarian_s_enchanted_book_is_read_back_whole() {
        let set = profession_trade_set("minecraft:librarian", 1).unwrap();
        let names: Vec<&str> = set
            .trades
            .iter()
            .map(|&id| villager_trade(id).name)
            .collect();
        assert_eq!(
            names,
            [
                "minecraft:librarian/1/paper_emerald",
                "minecraft:librarian/1/emerald_and_book_enchanted_book",
                "minecraft:librarian/1/emerald_bookshelf",
            ]
        );
        let id = trade_id_by_name("minecraft:librarian/1/emerald_and_book_enchanted_book").unwrap();
        // Item ids fit 16 bits, as the tables keep them.
        const EMERALD: u16 = items::EMERALD as u16;
        const BOOK: u16 = items::BOOK as u16;
        const ENCHANTED_BOOK: u16 = items::ENCHANTED_BOOK as u16;
        assert_eq!(
            *villager_trade(id),
            VillagerTrade {
                name: "minecraft:librarian/1/emerald_and_book_enchanted_book",
                wants: TradeCost {
                    item: EMERALD,
                    count: ContextIntProvider::Constant(0),
                    components: &[],
                },
                additional_wants: Some(TradeCost {
                    item: BOOK,
                    count: ContextIntProvider::Constant(1),
                    components: &[],
                }),
                gives: TradeItem {
                    item: ENCHANTED_BOOK,
                    count: 1,
                    components: &[],
                },
                max_uses: ContextIntProvider::Constant(12),
                xp: ContextIntProvider::Constant(1),
                reputation_discount: 0.2,
                given_item_modifier: &[
                    TradeModifier::EnchantRandomly {
                        options: Some(NamedSet::Tag("#minecraft:tradeable")),
                        only_compatible: false,
                        include_additional_cost_component: true,
                    },
                    TradeModifier::Filtered {
                        item_filter: ItemFilter {
                            items: Some(ItemSet::Listed(&[ENCHANTED_BOOK])),
                            predicates: &[ItemSubPredicate::StoredEnchantments(&[
                                EnchantmentPredicate {
                                    enchantments: None,
                                    min_level: None,
                                    max_level: None,
                                }
                            ])],
                        },
                        on_pass: &[],
                        on_fail: &[TradeModifier::Discard],
                    },
                ],
                merchant_predicate: None,
                double_trade_price_enchantments: Some(NamedSet::Tag(
                    "#minecraft:double_trade_price"
                )),
            }
        );
        let paper = villager_trade(set.trades[0]);
        assert_eq!(paper.wants.count, ContextIntProvider::Constant(24));
        assert_eq!(paper.xp, ContextIntProvider::Constant(2));
        assert_eq!(paper.reputation_discount, 0.05);
    }

    /// What the census of D10 B.5 counts: 27 trades for some villager types, 4 that
    /// double, 18 that want a second item, and the modifiers by the kind of the first;
    /// the wandering trader's water bottle wanted by its potion.
    #[test]
    fn the_trades_hold_what_the_census_counts() {
        let trades = villager_trades();
        let count =
            |test: &dyn Fn(&VillagerTrade) -> bool| trades.iter().filter(|t| test(t)).count();
        assert_eq!(count(&|t| t.merchant_predicate.is_some()), 27);
        assert_eq!(count(&|t| t.double_trade_price_enchantments.is_some()), 4);
        assert_eq!(count(&|t| t.additional_wants.is_some()), 18);
        assert_eq!(count(&|t| !t.given_item_modifier.is_empty()), 41);
        let first = |kind: fn(&TradeModifier) -> bool| {
            count(&|t: &VillagerTrade| t.given_item_modifier.first().is_some_and(kind))
        };
        assert_eq!(
            first(|m| matches!(m, TradeModifier::EnchantWithLevels { .. })),
            17
        );
        assert_eq!(
            first(|m| matches!(m, TradeModifier::ExplorationMap { .. })),
            10
        );
        assert_eq!(
            first(|m| matches!(m, TradeModifier::SetRandomDyes { .. })),
            7
        );
        assert_eq!(
            first(|m| matches!(m, TradeModifier::EnchantRandomly { .. })),
            4
        );
        assert_eq!(first(|m| matches!(m, TradeModifier::SetPotion { .. })), 1);
        assert_eq!(
            first(|m| matches!(m, TradeModifier::SetRandomPotion { .. })),
            1
        );
        assert_eq!(
            first(|m| matches!(m, TradeModifier::SetStewEffect { .. })),
            1
        );
        let water: Vec<&VillagerTrade> = trades
            .iter()
            .filter(|t| !t.wants.components.is_empty())
            .collect();
        assert_eq!(water.len(), 1);
        assert_eq!(
            water[0].wants.components,
            &[TradeComponent::PotionContents {
                potion: "minecraft:water"
            }]
        );
        // Every id names an item, and every set's trades are there.
        for trade in trades {
            assert!(usize::from(trade.wants.item) < crate::ITEMS.len());
            assert!(usize::from(trade.gives.item) < crate::ITEMS.len());
            assert!((1..=99).contains(&trade.gives.count));
        }
        for set in trade_sets() {
            assert!(set.trades.iter().all(|&id| usize::from(id) < trades.len()));
        }
    }
}
