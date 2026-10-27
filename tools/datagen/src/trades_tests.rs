//! The trades' compiler on small files written here, and on the data generator's output
//! where `cargo datagen` left it.

use super::*;

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// The registries' names a [`Names`] borrows.
struct Lists {
    items: Vec<String>,
    potions: Vec<String>,
    mob_effects: Vec<String>,
    enchantments: Vec<String>,
    villager_types: Vec<String>,
    map_decorations: Vec<String>,
    enchantment_tags: Vec<String>,
    potion_tags: Vec<String>,
}

fn lists() -> Lists {
    Lists {
        items: strings(&[
            "minecraft:air",
            "minecraft:emerald",
            "minecraft:book",
            "minecraft:enchanted_book",
            "minecraft:paper",
            "minecraft:potion",
            "minecraft:leather_helmet",
            "minecraft:iron_sword",
            "minecraft:suspicious_stew",
            "minecraft:tipped_arrow",
            "minecraft:arrow",
            "minecraft:map",
            "minecraft:spruce_boat",
        ]),
        potions: strings(&["minecraft:water", "minecraft:long_invisibility"]),
        mob_effects: strings(&["minecraft:night_vision", "minecraft:poison"]),
        enchantments: strings(&["minecraft:sharpness", "minecraft:mending"]),
        villager_types: strings(&["minecraft:snow", "minecraft:taiga"]),
        map_decorations: strings(&["minecraft:mansion", "minecraft:monument"]),
        enchantment_tags: strings(&[
            "#minecraft:tradeable",
            "#minecraft:double_trade_price",
            "#minecraft:on_traded_equipment",
        ]),
        potion_tags: strings(&["#minecraft:tradeable"]),
    }
}

impl Lists {
    fn names(&self) -> Names<'_> {
        Names {
            items: &self.items,
            potions: &self.potions,
            mob_effects: &self.mob_effects,
            enchantments: &self.enchantments,
            villager_types: &self.villager_types,
            map_decorations: &self.map_decorations,
            enchantment_tags: &self.enchantment_tags,
            potion_tags: &self.potion_tags,
        }
    }
}

fn files(listed: &[(&str, &str)]) -> Vec<(String, String)> {
    listed
        .iter()
        .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
        .collect()
}

/// `villager_trade/librarian/1/emerald_and_book_enchanted_book.json` as the game has it.
const BOOK: &str = r##"{
  "additional_wants": {"id": "minecraft:book"},
  "double_trade_price_enchantments": "#minecraft:double_trade_price",
  "given_item_modifier": [
    {"type": "minecraft:enchant_randomly", "include_additional_cost_component": true,
     "only_compatible": false, "options": "#minecraft:tradeable"},
    {"type": "minecraft:filtered", "item_filter": {"items": "minecraft:enchanted_book",
     "predicates": {"minecraft:stored_enchantments": [{}]}},
     "on_fail": {"type": "minecraft:discard"}}
  ],
  "gives": {"id": "minecraft:enchanted_book"},
  "max_uses": 12,
  "reputation_discount": 0.2,
  "wants": {"count": 0, "id": "minecraft:emerald"}
}"##;

/// A trade that leaves everything it can to the defaults.
const PAPER: &str = r#"{"wants": {"id": "minecraft:paper"}, "gives": {"id": "minecraft:emerald"}}"#;

fn compile_one(text: &str) -> Result<Trade> {
    let lists = lists();
    let mut compiled = compile(
        &files(&[("minecraft:librarian/1/x", text)]),
        &[],
        &[],
        &lists.names(),
    )?;
    Ok(compiled.trades.remove(0))
}

#[test]
fn what_a_trade_leaves_out_is_the_codec_s_default() {
    assert_eq!(
        compile_one(PAPER).unwrap(),
        Trade {
            name: "minecraft:librarian/1/x".to_owned(),
            // `TradeCost`'s count of 1.
            wants: Cost {
                item: 4,
                count: IntProvider::Constant(1),
                components: vec![],
            },
            additional_wants: None,
            // `ItemStackTemplate`'s count of 1.
            gives: Given {
                item: 1,
                count: 1,
                components: vec![],
            },
            // `VillagerTrade`'s `exactly(4)`, `exactly(1)` and `exactly(0f)`.
            max_uses: IntProvider::Constant(4),
            xp: IntProvider::Constant(1),
            reputation_discount: 0.0f32.to_bits(),
            given_item_modifier: vec![],
            merchant_predicate: None,
            double_trade_price_enchantments: None,
        }
    );
}

#[test]
fn the_librarian_s_enchanted_book_is_compiled_whole() {
    let trade = compile_one(BOOK).unwrap();
    assert_eq!(trade.wants.count, IntProvider::Constant(0));
    assert_eq!(
        trade.additional_wants,
        Some(Cost {
            item: 2,
            count: IntProvider::Constant(1),
            components: vec![]
        })
    );
    assert_eq!(trade.gives.item, 3);
    assert_eq!(trade.max_uses, IntProvider::Constant(12));
    assert_eq!(trade.xp, IntProvider::Constant(1));
    assert_eq!(trade.reputation_discount, 0.2f32.to_bits());
    assert_eq!(
        trade.double_trade_price_enchantments,
        Some(NamedSet::Tag("#minecraft:double_trade_price".to_owned()))
    );
    assert_eq!(
        trade.given_item_modifier,
        [
            Modifier::EnchantRandomly {
                options: Some(NamedSet::Tag("#minecraft:tradeable".to_owned())),
                only_compatible: false,
                include_additional_cost_component: true,
            },
            Modifier::Filtered {
                item_filter: ItemFilter {
                    items: Some(ItemSet::Listed(vec![3])),
                    predicates: vec![SubPredicate::StoredEnchantments(vec![
                        EnchantmentPredicate {
                            enchantments: None,
                            min_level: None,
                            max_level: None,
                        }
                    ])],
                },
                on_pass: vec![],
                on_fail: vec![Modifier::Discard],
            },
        ]
    );
}

/// Each modifier the game's trades use, and `set_count`, with every field it has and the
/// codec's defaults for those it leaves out.
#[test]
fn each_kind_of_modifier_is_compiled_with_its_fields_and_defaults() {
    let with = |modifier: &str| -> Result<Vec<Modifier>> {
        let text = PAPER.replace(
            "\"gives\"",
            &format!("\"given_item_modifier\": {modifier}, \"gives\""),
        );
        Ok(compile_one(&text)?.given_item_modifier)
    };
    let cases: Vec<(&str, Modifier)> = vec![
        (
            r#"{"type": "minecraft:enchant_randomly"}"#,
            Modifier::EnchantRandomly {
                options: None,
                only_compatible: true,
                include_additional_cost_component: false,
            },
        ),
        (
            r##"{"type": "minecraft:enchant_with_levels", "include_additional_cost_component":
              true, "levels": {"type": "minecraft:uniform", "max": 19, "min": 5},
              "options": "#minecraft:on_traded_equipment"}"##,
            Modifier::EnchantWithLevels {
                levels: IntProvider::Uniform { min: 5, max: 19 },
                options: Some(NamedSet::Tag("#minecraft:on_traded_equipment".to_owned())),
                include_additional_cost_component: true,
            },
        ),
        (
            r#"{"type": "minecraft:enchant_with_levels", "levels": 30,
              "options": ["minecraft:sharpness", "minecraft:mending"]}"#,
            Modifier::EnchantWithLevels {
                levels: IntProvider::Constant(30),
                options: Some(NamedSet::Listed(strings(&[
                    "minecraft:sharpness",
                    "minecraft:mending",
                ]))),
                include_additional_cost_component: false,
            },
        ),
        (
            r#"{"type": "minecraft:set_random_dyes", "number_of_dyes": {"type":
              "minecraft:add", "inputs": [1, {"type": "minecraft:binomial", "n": 2,
              "p": 0.75}]}}"#,
            Modifier::SetRandomDyes {
                number_of_dyes: IntProvider::Add(vec![
                    IntProvider::Constant(1),
                    IntProvider::Binomial {
                        n: 2,
                        p: 0.75f32.to_bits(),
                    },
                ]),
            },
        ),
        (
            r##"{"type": "minecraft:set_random_potion", "options": "#minecraft:tradeable"}"##,
            Modifier::SetRandomPotion {
                options: Some(NamedSet::Tag("#minecraft:tradeable".to_owned())),
            },
        ),
        (
            r#"{"type": "minecraft:set_stew_effect", "effects": [{"type":
              "minecraft:night_vision", "duration": 5}, {"type": "minecraft:poison",
              "duration": {"type": "minecraft:constant", "value": 14}}]}"#,
            Modifier::SetStewEffect {
                effects: vec![
                    StewEffect {
                        effect: "minecraft:night_vision".to_owned(),
                        duration: IntProvider::Constant(5),
                    },
                    StewEffect {
                        effect: "minecraft:poison".to_owned(),
                        duration: IntProvider::Constant(14),
                    },
                ],
            },
        ),
        (
            r#"{"type": "minecraft:set_stew_effect"}"#,
            Modifier::SetStewEffect { effects: vec![] },
        ),
        (
            r#"{"type": "minecraft:set_potion", "id": "minecraft:long_invisibility"}"#,
            Modifier::SetPotion {
                potion: "minecraft:long_invisibility".to_owned(),
            },
        ),
        (
            r#"{"type": "minecraft:set_count", "count": {"type": "minecraft:uniform",
              "min": 1, "max": 3}, "add": true}"#,
            Modifier::SetCount {
                count: IntProvider::Uniform { min: 1, max: 3 },
                add: true,
            },
        ),
        (
            r#"{"type": "minecraft:set_count", "count": 2}"#,
            Modifier::SetCount {
                count: IntProvider::Constant(2),
                add: false,
            },
        ),
        (
            r##"{"type": "minecraft:exploration_map", "decoration": "minecraft:monument",
              "destination": "#minecraft:on_ocean_explorer_maps", "search_radius": 100,
              "zoom": 3, "skip_existing_chunks": false}"##,
            Modifier::ExplorationMap {
                destination: NamedSet::Tag("#minecraft:on_ocean_explorer_maps".to_owned()),
                decoration: "minecraft:monument".to_owned(),
                zoom: 3,
                search_radius: 100,
                skip_existing_chunks: false,
            },
        ),
        (
            // The woodland mansion's map leaves the decoration to the default.
            r##"{"type": "minecraft:exploration_map",
              "destination": "#minecraft:on_woodland_explorer_maps"}"##,
            Modifier::ExplorationMap {
                destination: NamedSet::Tag("#minecraft:on_woodland_explorer_maps".to_owned()),
                decoration: "minecraft:mansion".to_owned(),
                zoom: 2,
                search_radius: 50,
                skip_existing_chunks: true,
            },
        ),
        (
            r#"{"type": "minecraft:filtered", "item_filter": {"predicates":
              {"minecraft:map_id": {}}}, "on_pass": [{"type": "minecraft:set_count",
              "count": 1}], "on_fail": {"type": "minecraft:discard"}}"#,
            Modifier::Filtered {
                item_filter: ItemFilter {
                    items: None,
                    predicates: vec![SubPredicate::MapId],
                },
                on_pass: vec![Modifier::SetCount {
                    count: IntProvider::Constant(1),
                    add: false,
                }],
                on_fail: vec![Modifier::Discard],
            },
        ),
        (
            r##"{"type": "minecraft:filtered", "item_filter": {"items":
              "#minecraft:swords", "predicates": {"minecraft:dyed_color": {},
              "minecraft:enchantments": [{"enchantments": "minecraft:sharpness",
              "levels": {"min": 2}}, {"levels": 1}]}}}"##,
            Modifier::Filtered {
                item_filter: ItemFilter {
                    items: Some(ItemSet::Tag("#minecraft:swords".to_owned())),
                    predicates: vec![
                        SubPredicate::DyedColor,
                        SubPredicate::Enchantments(vec![
                            EnchantmentPredicate {
                                enchantments: Some(NamedSet::Listed(strings(&[
                                    "minecraft:sharpness",
                                ]))),
                                min_level: Some(2),
                                max_level: None,
                            },
                            EnchantmentPredicate {
                                enchantments: None,
                                min_level: Some(1),
                                max_level: Some(1),
                            },
                        ]),
                    ],
                },
                on_pass: vec![],
                on_fail: vec![],
            },
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(
            with(text).unwrap(),
            std::slice::from_ref(&expected),
            "{text}"
        );
        // One modifier alone and a list of one are the same.
        assert_eq!(with(&format!("[{text}]")).unwrap(), [expected], "{text}");
    }
}

#[test]
fn a_merchant_predicate_and_a_wanted_potion_are_compiled() {
    let trade = compile_one(
        r#"{"wants": {"id": "minecraft:potion", "components": {"minecraft:potion_contents":
          {"potion": "minecraft:water"}}}, "gives": {"id": "minecraft:emerald", "count": 2},
          "merchant_predicate": {"type": "minecraft:entity_properties", "entity": "this",
          "predicate": {"minecraft:predicates": {"minecraft:villager/variant":
          ["minecraft:taiga", "minecraft:snow"]}}}, "xp": 30, "reputation_discount": 0.05}"#,
    )
    .unwrap();
    assert_eq!(
        trade.wants.components,
        [Component::PotionContents {
            potion: "minecraft:water".to_owned()
        }]
    );
    assert_eq!(trade.gives.count, 2);
    assert_eq!(trade.xp, IntProvider::Constant(30));
    assert_eq!(
        trade.merchant_predicate,
        Some(strings(&["minecraft:taiga", "minecraft:snow"]))
    );
    let single = compile_one(
        r#"{"wants": {"id": "minecraft:spruce_boat"}, "gives": {"id": "minecraft:emerald"},
          "merchant_predicate": {"type": "minecraft:entity_properties", "entity": "this",
          "predicate": {"minecraft:predicates": {"minecraft:villager/variant":
          "minecraft:snow"}}}}"#,
    )
    .unwrap();
    assert_eq!(
        single.merchant_predicate,
        Some(strings(&["minecraft:snow"]))
    );
}

/// A set's tag: a nested tag expanded where it stands, in its own order; a trade named
/// twice kept where it first came; an optional entry that is not there left out; the
/// sets and the trades each sorted by name.
#[test]
fn a_set_s_tag_is_expanded_in_order_with_nested_tags_in_place() {
    let lists = lists();
    let trades = files(&[
        ("minecraft:armorer/1/c", PAPER),
        ("minecraft:common_smith/1/b", PAPER),
        ("minecraft:armorer/1/a", BOOK),
        ("minecraft:common_smith/1/d", PAPER),
    ]);
    let tags = files(&[
        (
            "minecraft:armorer/level_1",
            r##"{"values": ["#minecraft:common_smith/level_1", "minecraft:armorer/1/c",
              "minecraft:common_smith/1/b", {"id": "minecraft:armorer/1/gone",
              "required": false}, {"id": "#minecraft:gone", "required": false},
              "minecraft:armorer/1/a"]}"##,
        ),
        (
            "minecraft:common_smith/level_1",
            r#"{"values": ["minecraft:common_smith/1/d", "minecraft:common_smith/1/b"]}"#,
        ),
    ]);
    let sets = files(&[
        (
            "minecraft:wandering_trader/common",
            r#"{"amount": 5, "random_sequence": "minecraft:trade_set/wandering_trader/common",
              "trades": ["minecraft:armorer/1/a"], "allow_duplicates": true}"#,
        ),
        (
            "minecraft:armorer/level_1",
            r##"{"amount": 2, "random_sequence": "minecraft:trade_set/armorer/level_1",
              "trades": "#minecraft:armorer/level_1"}"##,
        ),
    ]);
    let compiled = compile(&trades, &tags, &sets, &lists.names()).unwrap();
    let names: Vec<&str> = compiled
        .trades
        .iter()
        .map(|trade| trade.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "minecraft:armorer/1/a",
            "minecraft:armorer/1/c",
            "minecraft:common_smith/1/b",
            "minecraft:common_smith/1/d",
        ]
    );
    assert_eq!(
        compiled.sets,
        [
            TradeSet {
                name: "minecraft:armorer/level_1".to_owned(),
                amount: IntProvider::Constant(2),
                allow_duplicates: false,
                random_sequence: Some("minecraft:trade_set/armorer/level_1".to_owned()),
                // d and b (the nested tag), c, then a; b named again stays first.
                trades: vec![3, 2, 1, 0],
            },
            TradeSet {
                name: "minecraft:wandering_trader/common".to_owned(),
                amount: IntProvider::Constant(5),
                allow_duplicates: true,
                random_sequence: Some("minecraft:trade_set/wandering_trader/common".to_owned()),
                trades: vec![0],
            },
        ]
    );

    // A required trade or tag that is not there, a tag that names itself, a key nobody
    // reads.
    for broken in [
        r#"{"values": ["minecraft:armorer/1/gone"]}"#,
        r##"{"values": ["#minecraft:gone"]}"##,
        r##"{"values": ["#minecraft:armorer/level_1"]}"##,
        r#"{"values": ["minecraft:armorer/1/a"], "extra": 1}"#,
    ] {
        let tags = files(&[("minecraft:armorer/level_1", broken)]);
        assert!(
            compile(&trades, &tags, &sets[1..], &lists.names()).is_err(),
            "{broken}"
        );
    }
    // A set without an amount, or with a key nobody reads.
    for broken in [
        r##"{"trades": "#minecraft:armorer/level_1"}"##,
        r##"{"amount": 2, "trades": "#minecraft:armorer/level_1", "weight": 1}"##,
    ] {
        let sets = files(&[("minecraft:armorer/level_1", broken)]);
        assert!(
            compile(&trades, &tags, &sets, &lists.names()).is_err(),
            "{broken}"
        );
    }
}

/// Anything the compiler does not understand fails it, so that a jar that adds it is
/// noticed.
#[test]
fn what_is_not_understood_is_refused() {
    let refused = [
        // A key nobody reads, in a trade, in a cost, in a modifier.
        BOOK.replace("\"max_uses\"", "\"max_trades\""),
        BOOK.replace(
            "{\"count\": 0, \"id\"",
            "{\"count\": 0, \"price\": 1, \"id\"",
        ),
        BOOK.replace(
            "\"only_compatible\"",
            "\"conditions\": [], \"only_compatible\"",
        ),
        // Another kind of modifier, of provider, of item predicate, of component.
        BOOK.replace("minecraft:enchant_randomly", "minecraft:set_name"),
        BOOK.replace(
            "\"max_uses\": 12",
            "\"max_uses\": {\"type\": \"minecraft:clamped\"}",
        ),
        BOOK.replace(
            "\"max_uses\": 12",
            "\"max_uses\": \"minecraft:compostable/low\"",
        ),
        BOOK.replace("minecraft:stored_enchantments", "minecraft:damage"),
        BOOK.replace(
            "{\"id\": \"minecraft:book\"}",
            "{\"id\": \"minecraft:book\", \"components\": {\"minecraft:damage\": 1}}",
        ),
        // A reputation discount that is drawn.
        BOOK.replace(
            "\"reputation_discount\": 0.2",
            "\"reputation_discount\": {\"type\": \"minecraft:uniform\", \"min\": 0, \"max\": 1}",
        ),
        // Names that are no item, no enchantment tag, no enchantment.
        BOOK.replace("minecraft:book\"", "minecraft:tome\""),
        BOOK.replace("#minecraft:tradeable", "#minecraft:sellable"),
        BOOK.replace("#minecraft:double_trade_price", "minecraft:unbreaking"),
        // A count that a stack cannot have.
        BOOK.replace(
            "{\"id\": \"minecraft:enchanted_book\"}",
            "{\"id\": \"minecraft:enchanted_book\", \"count\": 100}",
        ),
        // Something in a part of a predicate that reads nothing else.
        BOOK.replace("[{}]", "[{\"count\": 1}]"),
        // A required part left out.
        BOOK.replace("\"gives\"", "\"given\""),
    ];
    for (index, text) in refused.iter().enumerate() {
        assert_ne!(text, BOOK, "case {index} changed nothing");
        assert!(compile_one(text).is_err(), "case {index} was accepted");
    }
    let predicate = |part: &str| {
        PAPER.replace(
            "\"gives\"",
            &format!("\"merchant_predicate\": {part}, \"gives\""),
        )
    };
    for part in [
        // Another entity, another predicate, a variant that is not one, another type.
        r#"{"type": "minecraft:entity_properties", "entity": "attacker", "predicate":
          {"minecraft:predicates": {"minecraft:villager/variant": "minecraft:snow"}}}"#,
        r#"{"type": "minecraft:entity_properties", "entity": "this", "predicate":
          {"minecraft:flags": {"is_baby": true}}}"#,
        r#"{"type": "minecraft:entity_properties", "entity": "this", "predicate":
          {"minecraft:predicates": {"minecraft:villager/variant": "minecraft:arctic"}}}"#,
        r#"{"type": "minecraft:random_chance", "chance": 0.5}"#,
        "{}",
    ] {
        assert!(compile_one(&predicate(part)).is_err(), "{part}");
    }
}

#[test]
fn the_emitted_tables_name_what_they_use_and_nothing_else() {
    let lists = lists();
    let compiled = compile(
        &files(&[
            ("minecraft:librarian/1/b", BOOK),
            ("minecraft:librarian/1/p", PAPER),
        ]),
        &files(&[(
            "minecraft:librarian/level_1",
            r#"{"values": ["minecraft:librarian/1/p", "minecraft:librarian/1/b"]}"#,
        )]),
        &files(&[(
            "minecraft:librarian/level_1",
            r##"{"amount": 2, "trades": "#minecraft:librarian/level_1"}"##,
        )]),
        &lists.names(),
    )
    .unwrap();
    let text = emit(&compiled).unwrap();
    assert!(
        text.starts_with(
            "use crate::trading::{ContextIntProvider, EnchantmentPredicate, ItemFilter, \
             ItemSet, ItemSubPredicate, NamedSet, TradeCost, TradeItem, TradeModifier, \
             TradeSet, VillagerTrade};\n\n"
        ),
        "{text}"
    );
    assert!(text.contains("pub static VILLAGER_TRADES: [VillagerTrade; 2] = ["));
    assert!(text.contains(
        "    TradeSet { name: \"minecraft:librarian/level_1\", amount: \
         ContextIntProvider::Constant(2), allow_duplicates: false, random_sequence: None, \
         trades: &[1, 0] },"
    ));
    assert!(text.contains(
        "    VillagerTrade { name: \"minecraft:librarian/1/p\", wants: TradeCost { item: 4, \
         count: ContextIntProvider::Constant(1), components: &[] }, additional_wants: None, \
         gives: TradeItem { item: 1, count: 1, components: &[] }, max_uses: \
         ContextIntProvider::Constant(4), xp: ContextIntProvider::Constant(1), \
         reputation_discount: f32::from_bits(0x00000000), given_item_modifier: &[], \
         merchant_predicate: None, double_trade_price_enchantments: None },\n"
    ));
    assert!(text.contains("reputation_discount: f32::from_bits(0x3e4ccccd),"));
    assert!(text.contains(
        "TradeModifier::Filtered { item_filter: ItemFilter { items: \
         Some(ItemSet::Listed(&[3])), predicates: \
         &[ItemSubPredicate::StoredEnchantments(&[EnchantmentPredicate { enchantments: \
         None, min_level: None, max_level: None }])] }, on_pass: &[], on_fail: \
         &[TradeModifier::Discard] }"
    ));

    // With no trades, the two tables are empty and name only their own types.
    assert_eq!(
        emit(&Compiled::default()).unwrap(),
        "use crate::trading::{TradeSet, VillagerTrade};\n\n\
         /// The villagers' and the wandering trader's trades, by name.\n\
         pub static VILLAGER_TRADES: [VillagerTrade; 0] = [\n];\n\n\
         /// The trade sets, by name, each naming its trades by their numbers in \
         `VILLAGER_TRADES`.\n\
         pub static TRADE_SETS: [TradeSet; 0] = [\n];\n"
    );
}

/// Reads every trade, trade tag and trade set of the data generator's output that
/// `cargo datagen` keeps in `target/datagen`; needs no Java. Run it with
/// `cargo test -p clustine-datagen -- --ignored generated_trades`; with
/// `CLUSTINE_TRADES_OUT` set, it writes the emitted table there.
#[test]
#[ignore = "needs the data generator's output in target/datagen"]
fn every_trade_of_the_generated_trades_is_understood() {
    use crate::model;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let generated = root
        .join("target/datagen")
        .join(crate::jar::SERVER_JAR.version)
        .join("generated");
    let data = generated.join("data/minecraft");
    let registries = model::read_json(&generated.join("reports/registries.json")).unwrap();
    let registry = |name: &str| model::static_registry(&registries, name).unwrap();
    let entries = |directory: &str| -> Vec<String> {
        tables_under(&data.join(directory))
            .unwrap()
            .into_iter()
            .map(|path| format!("minecraft:{path}"))
            .collect()
    };
    let tags = |directory: &str| -> Vec<String> {
        entries(directory)
            .into_iter()
            .map(|name| format!("#{name}"))
            .collect()
    };
    let lists = Lists {
        items: registry("item"),
        potions: registry("potion"),
        mob_effects: registry("mob_effect"),
        enchantments: entries("enchantment"),
        villager_types: registry("villager_type"),
        map_decorations: registry("map_decoration_type"),
        enchantment_tags: tags("tags/enchantment"),
        potion_tags: tags("tags/potion"),
    };
    let compiled = load(&data, &lists.names()).unwrap();
    assert_eq!(compiled.trades.len(), 391);
    assert_eq!(compiled.sets.len(), 68);
    let size = |name: &str| -> usize {
        compiled
            .sets
            .iter()
            .find(|set| set.name == name)
            .unwrap_or_else(|| panic!("no set {name}"))
            .trades
            .len()
    };
    assert_eq!(size("minecraft:armorer/level_1"), 5);
    assert_eq!(size("minecraft:mason/level_4"), 33);
    assert_eq!(size("minecraft:shepherd/level_2"), 37);
    // The trader's (V 5.2): buying 6, uncommon 16, common 78.
    assert_eq!(size("minecraft:wandering_trader/buying"), 6);
    assert_eq!(size("minecraft:wandering_trader/uncommon"), 16);
    assert_eq!(size("minecraft:wandering_trader/common"), 78);
    let text = emit(&compiled).unwrap();
    println!("{} bytes", text.len());
    if let Some(out) = std::env::var_os("CLUSTINE_TRADES_OUT") {
        fs::write(out, text).unwrap();
    }
}
