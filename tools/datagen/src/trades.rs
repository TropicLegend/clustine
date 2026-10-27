//! Compiles the villagers' and the wandering trader's trades into `trades.rs`, which
//! `clustine-data`'s `trading` reads (`docs/groundwork/n10-brief.md`, section 5.2;
//! `docs/groundwork/n10-dumps.md`, part B, sections 5 and 8).
//!
//! The trades are `data/minecraft/villager_trade/**/*.json`, each named by its path
//! (`minecraft:librarian/1/paper_emerald`); the sets `data/minecraft/trade_set/**/*.json`
//! (`minecraft:armorer/level_1`); and the trade tags the sets name,
//! `data/minecraft/tags/villager_trade/**/*.json`, whose entries are kept in order with a
//! nested tag expanded where it stands and each trade kept where it first comes, as the
//! game's tag loader builds a tag. Every default is the codec's (`VillagerTrade`,
//! `TradeCost`, `ItemStackTemplate`, `TradeSet` and the loot functions', read in the
//! dumps), so that the reader never guesses one.
//!
//! As the loot compiler does, this understands only what the game's trades use, and
//! anything else (a key no reader here knows, another kind of modifier, provider,
//! predicate or component, a name that is no item, potion, effect, enchantment or
//! villager type) fails the run, so that a jar that adds one is noticed.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;
use crate::loot::{float_bits, kind, only, tables_under};

/// `ContextIntProvider`: a whole number, drawn or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntProvider {
    Constant(i32),
    Uniform {
        min: i32,
        max: i32,
    },
    /// `p` as float bits.
    Binomial {
        n: i32,
        p: u32,
    },
    Add(Vec<IntProvider>),
}

/// A `HolderSet` named by a tag (with `#`) or by its entries' names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamedSet {
    Tag(String),
    Listed(Vec<String>),
}

/// A `HolderSet` of items: a tag, or items by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemSet {
    Tag(String),
    Listed(Vec<usize>),
}

/// A component value, as an exact predicate or a template has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Component {
    PotionContents { potion: String },
}

/// `TradeCost`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cost {
    pub item: usize,
    pub count: IntProvider,
    pub components: Vec<Component>,
}

/// `ItemStackTemplate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Given {
    pub item: usize,
    pub count: i32,
    pub components: Vec<Component>,
}

/// `EnchantmentPredicate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnchantmentPredicate {
    pub enchantments: Option<NamedSet>,
    pub min_level: Option<i32>,
    pub max_level: Option<i32>,
}

/// A part of an item predicate's `predicates`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubPredicate {
    Enchantments(Vec<EnchantmentPredicate>),
    StoredEnchantments(Vec<EnchantmentPredicate>),
    DyedColor,
    MapId,
}

/// The `ItemPredicate` of a `filtered`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemFilter {
    pub items: Option<ItemSet>,
    pub predicates: Vec<SubPredicate>,
}

/// An entry of `set_stew_effect`: the effect and its duration in seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StewEffect {
    pub effect: String,
    pub duration: IntProvider,
}

/// A loot item function as a trade applies it to what it gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modifier {
    EnchantRandomly {
        options: Option<NamedSet>,
        only_compatible: bool,
        include_additional_cost_component: bool,
    },
    EnchantWithLevels {
        levels: IntProvider,
        options: Option<NamedSet>,
        include_additional_cost_component: bool,
    },
    Filtered {
        item_filter: ItemFilter,
        on_pass: Vec<Modifier>,
        on_fail: Vec<Modifier>,
    },
    Discard,
    SetRandomDyes {
        number_of_dyes: IntProvider,
    },
    SetRandomPotion {
        options: Option<NamedSet>,
    },
    SetStewEffect {
        effects: Vec<StewEffect>,
    },
    SetPotion {
        potion: String,
    },
    SetCount {
        count: IntProvider,
        add: bool,
    },
    ExplorationMap {
        destination: NamedSet,
        decoration: String,
        zoom: i8,
        search_radius: i32,
        skip_existing_chunks: bool,
    },
}

/// `VillagerTrade`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trade {
    pub name: String,
    pub wants: Cost,
    pub additional_wants: Option<Cost>,
    pub gives: Given,
    pub max_uses: IntProvider,
    pub xp: IntProvider,
    /// As float bits.
    pub reputation_discount: u32,
    pub given_item_modifier: Vec<Modifier>,
    /// The villager types of an `entity_properties` on `this`.
    pub merchant_predicate: Option<Vec<String>>,
    pub double_trade_price_enchantments: Option<NamedSet>,
}

/// `TradeSet`, its trades by their numbers among the compiled trades.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeSet {
    pub name: String,
    pub amount: IntProvider,
    pub allow_duplicates: bool,
    pub random_sequence: Option<String>,
    pub trades: Vec<usize>,
}

/// The trades sorted by name, and the sets sorted by name.
#[derive(Debug, Default)]
pub struct Compiled {
    pub trades: Vec<Trade>,
    pub sets: Vec<TradeSet>,
}

/// The names the trades are checked against, each list a registry's by its ids (items)
/// or by name. The tags are the names, with `#`, of the enchantments' and potions'.
pub struct Names<'a> {
    pub items: &'a [String],
    pub potions: &'a [String],
    pub mob_effects: &'a [String],
    pub enchantments: &'a [String],
    pub villager_types: &'a [String],
    pub map_decorations: &'a [String],
    pub enchantment_tags: &'a [String],
    pub potion_tags: &'a [String],
}

/// The registries a `HolderSet` of a trade can be of.
#[derive(Clone, Copy)]
enum Registry {
    Enchantment,
    Potion,
    /// Structures, whose tags Clustine does not load; their names are only checked to
    /// be the game's.
    Structure,
}

impl Names<'_> {
    fn item(&self, name: &str) -> Result<usize> {
        self.items
            .iter()
            .position(|item| item == name)
            .with_context(|| format!("there is no item {name}"))
    }

    fn known(list: &[String], name: &str, what: &str) -> Result<String> {
        ensure!(
            list.iter().any(|known| known == name),
            "there is no {what} {name}"
        );
        Ok(name.to_owned())
    }

    fn named(&self, registry: Registry, name: &str) -> Result<String> {
        match (registry, name.strip_prefix('#')) {
            (Registry::Enchantment, Some(_)) => {
                Self::known(self.enchantment_tags, name, "enchantment tag")
            }
            (Registry::Enchantment, None) => Self::known(self.enchantments, name, "enchantment"),
            (Registry::Potion, Some(_)) => Self::known(self.potion_tags, name, "potion tag"),
            (Registry::Potion, None) => Self::known(self.potions, name, "potion"),
            (Registry::Structure, _) => {
                ensure!(
                    name.trim_start_matches('#').starts_with("minecraft:"),
                    "the structure {name} is not the game's"
                );
                Ok(name.to_owned())
            }
        }
    }
}

/// Reads the trades, the trade tags and the sets of `data` (`data/minecraft`).
pub fn load(data: &Path, names: &Names) -> Result<Compiled> {
    let read = |directory: &str| -> Result<Vec<(String, String)>> {
        let directory = data.join(directory);
        tables_under(&directory)?
            .into_iter()
            .map(|path| {
                let file = directory.join(format!("{path}.json"));
                let text = fs::read_to_string(&file)
                    .with_context(|| format!("reading {}", file.display()))?;
                Ok((format!("minecraft:{path}"), text))
            })
            .collect()
    };
    compile(
        &read("villager_trade")?,
        &read("tags/villager_trade")?,
        &read("trade_set")?,
        names,
    )
}

/// Compiles the trades, the trade tags and the sets, each a file's name and text.
pub fn compile(
    trades: &[(String, String)],
    tags: &[(String, String)],
    sets: &[(String, String)],
    names: &Names,
) -> Result<Compiled> {
    let mut compiled = Compiled::default();
    for (name, text) in trades {
        let json = Json::parse(text).with_context(|| format!("reading the trade {name}"))?;
        compiled
            .trades
            .push(trade(name, &json, names).with_context(|| format!("reading the trade {name}"))?);
    }
    compiled.trades.sort_by(|a, b| a.name.cmp(&b.name));
    let ids: BTreeMap<&str, usize> = compiled
        .trades
        .iter()
        .enumerate()
        .map(|(id, trade)| (trade.name.as_str(), id))
        .collect();

    let mut raw = BTreeMap::new();
    for (name, text) in tags {
        let json = Json::parse(text).with_context(|| format!("reading the trade tag {name}"))?;
        raw.insert(
            name.clone(),
            tag_members(&json).with_context(|| format!("reading the trade tag {name}"))?,
        );
    }

    for (name, text) in sets {
        let json = Json::parse(text).with_context(|| format!("reading the trade set {name}"))?;
        compiled.sets.push(
            trade_set(name, &json, &raw, &ids)
                .with_context(|| format!("reading the trade set {name}"))?,
        );
    }
    compiled.sets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(compiled)
}

/// A tag's members in order, each with whether it is required.
fn tag_members(json: &Json) -> Result<Vec<(String, bool)>> {
    only(json, &["values", "replace"])?;
    if let Some(replace) = json.get("replace") {
        ensure!(
            !replace.boolean()?,
            "a trade tag that replaces is not understood"
        );
    }
    let mut members = Vec::new();
    for value in json.member("values")?.array()? {
        members.push(match value {
            Json::String(id) => (id.clone(), true),
            _ => {
                only(value, &["id", "required"])?;
                (
                    value.member("id")?.string()?.to_owned(),
                    value.get("required").map_or(Ok(true), Json::boolean)?,
                )
            }
        });
    }
    Ok(members)
}

/// Appends the trades of the tag `tag` to `out` in order, a nested tag's where it
/// stands; a trade already there stays where it first came.
fn resolve_tag<'a>(
    tag: &'a str,
    raw: &'a BTreeMap<String, Vec<(String, bool)>>,
    ids: &BTreeMap<&str, usize>,
    stack: &mut Vec<&'a str>,
    out: &mut Vec<usize>,
) -> Result<()> {
    ensure!(!stack.contains(&tag), "the trade tag {tag} names itself");
    stack.push(tag);
    let members = raw
        .get(tag)
        .with_context(|| format!("there is no trade tag {tag}"))?;
    for (member, required) in members {
        if let Some(nested) = member.strip_prefix('#') {
            if raw.contains_key(nested) || *required {
                resolve_tag(nested, raw, ids, stack, out)
                    .with_context(|| format!("in the trade tag {tag}"))?;
            }
        } else if let Some(&id) = ids.get(member.as_str()) {
            if !out.contains(&id) {
                out.push(id);
            }
        } else if *required {
            bail!("the trade tag {tag} names {member}, which is no trade");
        }
    }
    stack.pop();
    Ok(())
}

fn trade_set(
    name: &str,
    json: &Json,
    raw: &BTreeMap<String, Vec<(String, bool)>>,
    ids: &BTreeMap<&str, usize>,
) -> Result<TradeSet> {
    only(
        json,
        &["trades", "amount", "allow_duplicates", "random_sequence"],
    )?;
    let mut trades = Vec::new();
    let mut add = |member: &str| -> Result<()> {
        match member.strip_prefix('#') {
            Some(tag) => resolve_tag(tag, raw, ids, &mut Vec::new(), &mut trades),
            None => {
                let id = *ids
                    .get(member)
                    .with_context(|| format!("there is no trade {member}"))?;
                if !trades.contains(&id) {
                    trades.push(id);
                }
                Ok(())
            }
        }
    };
    match json.member("trades")? {
        Json::Array(members) => {
            for member in members {
                let member = member.string()?;
                ensure!(!member.starts_with('#'), "a list of trades names no tag");
                add(member)?;
            }
        }
        member => add(member.string()?)?,
    }
    Ok(TradeSet {
        name: name.to_owned(),
        amount: int_provider(json.member("amount")?).context("the amount")?,
        allow_duplicates: json
            .get("allow_duplicates")
            .map_or(Ok(false), Json::boolean)?,
        random_sequence: json
            .get("random_sequence")
            .map(|sequence| sequence.string().map(str::to_owned))
            .transpose()?,
        trades,
    })
}

fn trade(name: &str, json: &Json, names: &Names) -> Result<Trade> {
    only(
        json,
        &[
            "wants",
            "additional_wants",
            "gives",
            "max_uses",
            "xp",
            "reputation_discount",
            "merchant_predicate",
            "given_item_modifier",
            "double_trade_price_enchantments",
        ],
    )?;
    let provider = |key: &str, default: i32| -> Result<IntProvider> {
        json.get(key)
            .map_or(Ok(IntProvider::Constant(default)), int_provider)
            .with_context(|| format!("the {key}"))
    };
    let mut given_item_modifier = Vec::new();
    if let Some(value) = json.get("given_item_modifier") {
        modifiers(value, names, &mut given_item_modifier)?;
    }
    Ok(Trade {
        name: name.to_owned(),
        wants: cost(json.member("wants")?, names).context("what it wants")?,
        additional_wants: json
            .get("additional_wants")
            .map(|value| cost(value, names))
            .transpose()
            .context("what else it wants")?,
        gives: given(json.member("gives")?, names).context("what it gives")?,
        // `VillagerTrade.CODEC`'s defaults: `exactly(4)`, `exactly(1)`, `exactly(0f)`.
        max_uses: provider("max_uses", 4)?,
        xp: provider("xp", 1)?,
        reputation_discount: json
            .get("reputation_discount")
            .map_or(Ok(0.0f32.to_bits()), float_provider)
            .context("the reputation discount")?,
        given_item_modifier,
        merchant_predicate: json
            .get("merchant_predicate")
            .map(|value| merchant_predicate(value, names))
            .transpose()
            .context("the merchant predicate")?,
        double_trade_price_enchantments: json
            .get("double_trade_price_enchantments")
            .map(|value| named_set(value, Registry::Enchantment, names))
            .transpose()?,
    })
}

/// `TradeCost`: `{id, count (a provider, 1 by default), components (none by default)}`.
fn cost(json: &Json, names: &Names) -> Result<Cost> {
    only(json, &["id", "count", "components"])?;
    Ok(Cost {
        item: names.item(json.member("id")?.string()?)?,
        count: json
            .get("count")
            .map_or(Ok(IntProvider::Constant(1)), int_provider)?,
        components: json
            .get("components")
            .map_or(Ok(Vec::new()), |value| components(value, names))?,
    })
}

/// `ItemStackTemplate`: `{id, count (1 to 99, 1 by default), components}`.
fn given(json: &Json, names: &Names) -> Result<Given> {
    only(json, &["id", "count", "components"])?;
    let count = json.get("count").map_or(Ok(1), Json::integer)?;
    ensure!(
        (1..=99).contains(&count),
        "a count of {count} is not from 1 to 99"
    );
    Ok(Given {
        item: names.item(json.member("id")?.string()?)?,
        count,
        components: json
            .get("components")
            .map_or(Ok(Vec::new()), |value| components(value, names))?,
    })
}

fn components(json: &Json, names: &Names) -> Result<Vec<Component>> {
    let mut read = Vec::new();
    for (key, value) in json.members()? {
        match key.as_str() {
            "minecraft:potion_contents" => {
                let potion = match value {
                    Json::String(potion) => potion.as_str(),
                    _ => {
                        only(value, &["potion"])?;
                        value.member("potion")?.string()?
                    }
                };
                read.push(Component::PotionContents {
                    potion: names.named(Registry::Potion, potion)?,
                });
            }
            other => bail!("the component {other} is not understood"),
        }
    }
    Ok(read)
}

/// `entity_properties` on `this` with the villager's variant.
fn merchant_predicate(json: &Json, names: &Names) -> Result<Vec<String>> {
    only(json, &["type", "entity", "predicate"])?;
    ensure!(
        kind(json)? == "entity_properties",
        "a merchant predicate of the type {} is not understood",
        kind(json)?
    );
    let entity = json.member("entity")?.string()?;
    ensure!(entity == "this", "the entity {entity} is not understood");
    let predicate = json.member("predicate")?;
    only(predicate, &["minecraft:predicates"])?;
    let predicates = predicate.member("minecraft:predicates")?;
    only(predicates, &["minecraft:villager/variant"])?;
    let variants = predicates.member("minecraft:villager/variant")?;
    let listed: Vec<&Json> = match variants {
        Json::Array(values) => values.iter().collect(),
        single => vec![single],
    };
    listed
        .into_iter()
        .map(|variant| {
            let variant = variant.string()?;
            ensure!(!variant.starts_with('#'), "a tag of villager types");
            Names::known(names.villager_types, variant, "villager type")
        })
        .collect()
}

/// The modifiers of `value`, a function or a list of them (a sequence), appended to
/// `out` in order. A sequence within a sequence is flattened, which applies the same.
fn modifiers(value: &Json, names: &Names, out: &mut Vec<Modifier>) -> Result<()> {
    match value {
        Json::Array(listed) => {
            for value in listed {
                modifiers(value, names, out)?;
            }
        }
        Json::Object(_) => out.push(modifier(value, names)?),
        other => bail!("a modifier that is {other:?} is not understood"),
    }
    Ok(())
}

fn modifier(json: &Json, names: &Names) -> Result<Modifier> {
    let options = |registry| -> Result<Option<NamedSet>> {
        json.get("options")
            .map(|value| named_set(value, registry, names))
            .transpose()
    };
    let flag = |key: &str, default: bool| json.get(key).map_or(Ok(default), Json::boolean);
    Ok(match kind(json)? {
        "enchant_randomly" => {
            only(
                json,
                &[
                    "type",
                    "options",
                    "only_compatible",
                    "include_additional_cost_component",
                ],
            )?;
            Modifier::EnchantRandomly {
                options: options(Registry::Enchantment)?,
                only_compatible: flag("only_compatible", true)?,
                include_additional_cost_component: flag(
                    "include_additional_cost_component",
                    false,
                )?,
            }
        }
        "enchant_with_levels" => {
            only(
                json,
                &[
                    "type",
                    "levels",
                    "options",
                    "include_additional_cost_component",
                ],
            )?;
            Modifier::EnchantWithLevels {
                levels: int_provider(json.member("levels")?).context("the levels")?,
                options: options(Registry::Enchantment)?,
                include_additional_cost_component: flag(
                    "include_additional_cost_component",
                    false,
                )?,
            }
        }
        "filtered" => {
            only(json, &["type", "item_filter", "on_pass", "on_fail"])?;
            let branch = |key: &str| -> Result<Vec<Modifier>> {
                let mut read = Vec::new();
                if let Some(value) = json.get(key) {
                    modifiers(value, names, &mut read).with_context(|| format!("in {key}"))?;
                }
                Ok(read)
            };
            Modifier::Filtered {
                item_filter: item_filter(json.member("item_filter")?, names)?,
                on_pass: branch("on_pass")?,
                on_fail: branch("on_fail")?,
            }
        }
        "discard" => {
            only(json, &["type"])?;
            Modifier::Discard
        }
        "set_random_dyes" => {
            only(json, &["type", "number_of_dyes"])?;
            Modifier::SetRandomDyes {
                number_of_dyes: int_provider(json.member("number_of_dyes")?)
                    .context("the number of dyes")?,
            }
        }
        "set_random_potion" => {
            only(json, &["type", "options"])?;
            Modifier::SetRandomPotion {
                options: options(Registry::Potion)?,
            }
        }
        "set_stew_effect" => {
            only(json, &["type", "effects"])?;
            let mut effects = Vec::new();
            if let Some(listed) = json.get("effects") {
                for entry in listed.array()? {
                    only(entry, &["type", "duration"])?;
                    effects.push(StewEffect {
                        effect: Names::known(
                            names.mob_effects,
                            entry.member("type")?.string()?,
                            "mob effect",
                        )?,
                        duration: int_provider(entry.member("duration")?)
                            .context("a stew's duration")?,
                    });
                }
            }
            Modifier::SetStewEffect { effects }
        }
        "set_potion" => {
            only(json, &["type", "id"])?;
            let potion = json.member("id")?.string()?;
            ensure!(!potion.starts_with('#'), "set_potion names one potion");
            Modifier::SetPotion {
                potion: names.named(Registry::Potion, potion)?,
            }
        }
        "set_count" => {
            only(json, &["type", "count", "add"])?;
            Modifier::SetCount {
                count: int_provider(json.member("count")?).context("the count")?,
                add: flag("add", false)?,
            }
        }
        "exploration_map" => {
            only(
                json,
                &[
                    "type",
                    "destination",
                    "decoration",
                    "zoom",
                    "search_radius",
                    "skip_existing_chunks",
                ],
            )?;
            let decoration = json
                .get("decoration")
                .map_or(Ok("minecraft:mansion"), Json::string)?;
            let zoom = json.get("zoom").map_or(Ok(2), Json::integer)?;
            Modifier::ExplorationMap {
                destination: named_set(json.member("destination")?, Registry::Structure, names)?,
                decoration: Names::known(names.map_decorations, decoration, "map decoration")?,
                zoom: i8::try_from(zoom).with_context(|| format!("a zoom of {zoom}"))?,
                search_radius: json.get("search_radius").map_or(Ok(50), Json::integer)?,
                skip_existing_chunks: flag("skip_existing_chunks", true)?,
            }
        }
        other => bail!("a trade's modifier of the type {other} is not understood"),
    })
}

/// `ItemPredicate` as a `filtered` has it: its items and its `predicates`.
fn item_filter(json: &Json, names: &Names) -> Result<ItemFilter> {
    only(json, &["items", "predicates"])?;
    let items = match json.get("items") {
        None => None,
        Some(Json::Array(listed)) => Some(ItemSet::Listed(
            listed
                .iter()
                .map(|item| names.item(item.string()?))
                .collect::<Result<Vec<_>>>()?,
        )),
        Some(single) => {
            let name = single.string()?;
            Some(if name.starts_with('#') {
                ItemSet::Tag(name.to_owned())
            } else {
                ItemSet::Listed(vec![names.item(name)?])
            })
        }
    };
    let mut predicates = Vec::new();
    if let Some(parts) = json.get("predicates") {
        for (key, value) in parts.members()? {
            predicates.push(match key.as_str() {
                "minecraft:enchantments" => {
                    SubPredicate::Enchantments(enchantment_predicates(value, names)?)
                }
                "minecraft:stored_enchantments" => {
                    SubPredicate::StoredEnchantments(enchantment_predicates(value, names)?)
                }
                "minecraft:dyed_color" => {
                    only(value, &[])?;
                    SubPredicate::DyedColor
                }
                "minecraft:map_id" => {
                    only(value, &[])?;
                    SubPredicate::MapId
                }
                other => bail!("the item predicate {other} is not understood"),
            });
        }
    }
    Ok(ItemFilter { items, predicates })
}

fn enchantment_predicates(json: &Json, names: &Names) -> Result<Vec<EnchantmentPredicate>> {
    json.array()?
        .iter()
        .map(|entry| {
            only(entry, &["enchantments", "levels"])?;
            let (min_level, max_level) = match entry.get("levels") {
                None => (None, None),
                Some(level @ Json::Number(_)) => (Some(level.integer()?), Some(level.integer()?)),
                Some(bounds) => {
                    only(bounds, &["min", "max"])?;
                    (
                        bounds.get("min").map(Json::integer).transpose()?,
                        bounds.get("max").map(Json::integer).transpose()?,
                    )
                }
            };
            Ok(EnchantmentPredicate {
                enchantments: entry
                    .get("enchantments")
                    .map(|value| named_set(value, Registry::Enchantment, names))
                    .transpose()?,
                min_level,
                max_level,
            })
        })
        .collect()
}

fn named_set(json: &Json, registry: Registry, names: &Names) -> Result<NamedSet> {
    Ok(match json {
        Json::Array(listed) => NamedSet::Listed(
            listed
                .iter()
                .map(|entry| {
                    let name = entry.string()?;
                    ensure!(!name.starts_with('#'), "a list names no tag");
                    names.named(registry, name)
                })
                .collect::<Result<Vec<_>>>()?,
        ),
        single => {
            let name = single.string()?;
            if name.starts_with('#') {
                NamedSet::Tag(names.named(registry, name)?)
            } else {
                NamedSet::Listed(vec![names.named(registry, name)?])
            }
        }
    })
}

/// `ContextIntProviders.CODEC`: a whole number, or `constant`, `uniform`, `binomial` or
/// `add` of such. A provider named by reference to the registry is not understood.
fn int_provider(value: &Json) -> Result<IntProvider> {
    if let Json::Number(_) = value {
        return Ok(IntProvider::Constant(value.integer()?));
    }
    ensure!(
        matches!(value, Json::Object(_)),
        "a provider that is {value:?} is not understood"
    );
    Ok(match kind(value)? {
        "constant" => {
            only(value, &["type", "value"])?;
            IntProvider::Constant(value.member("value")?.integer()?)
        }
        "uniform" => {
            only(value, &["type", "min", "max"])?;
            IntProvider::Uniform {
                min: value.member("min")?.integer()?,
                max: value.member("max")?.integer()?,
            }
        }
        "binomial" => {
            only(value, &["type", "n", "p"])?;
            IntProvider::Binomial {
                n: value.member("n")?.integer()?,
                p: float_bits(value.member("p")?)?,
            }
        }
        "add" => {
            only(value, &["type", "inputs"])?;
            IntProvider::Add(
                value
                    .member("inputs")?
                    .array()?
                    .iter()
                    .map(int_provider)
                    .collect::<Result<Vec<_>>>()?,
            )
        }
        other => bail!("a provider of the type {other} is not understood"),
    })
}

/// `ContextFloatProviders.CODEC` as the trades have it: a constant, as float bits.
fn float_provider(value: &Json) -> Result<u32> {
    if let Json::Number(_) = value {
        return float_bits(value);
    }
    ensure!(
        matches!(value, Json::Object(_)) && kind(value)? == "constant",
        "a float provider that is not a constant is not understood: {value:?}"
    );
    only(value, &["type", "value"])?;
    float_bits(value.member("value")?)
}

/// The types of `crate::trading` that the emitted file may name.
const TYPES: [&str; 14] = [
    "ContextIntProvider",
    "EnchantmentPredicate",
    "ItemFilter",
    "ItemSet",
    "ItemSubPredicate",
    "MerchantPredicate",
    "NamedSet",
    "StewEffect",
    "TradeComponent",
    "TradeCost",
    "TradeItem",
    "TradeModifier",
    "TradeSet",
    "VillagerTrade",
];

/// `trades.rs`: the trades by name, then the sets by name, importing only the types it
/// names (with no trades, a file that imported more would not build without a warning).
pub fn emit(compiled: &Compiled) -> Result<String> {
    ensure!(
        compiled.trades.len() < usize::from(u16::MAX),
        "more trades than 16 bits can number"
    );
    ensure!(
        compiled.trades.is_sorted_by(|a, b| a.name < b.name)
            && compiled.sets.is_sorted_by(|a, b| a.name < b.name),
        "the trades or the sets are not sorted by name"
    );
    let mut body = String::new();
    writeln!(
        body,
        "/// The villagers' and the wandering trader's trades, by name."
    )?;
    writeln!(
        body,
        "pub static VILLAGER_TRADES: [VillagerTrade; {}] = [",
        compiled.trades.len()
    )?;
    for trade in &compiled.trades {
        // A trade a line, as a loot table is, which keeps the file within its budget.
        writeln!(
            body,
            "    VillagerTrade {{ name: {:?}, wants: {}, additional_wants: {}, gives: \
             TradeItem {{ item: {}, count: {}, components: &[{}] }}, max_uses: {}, xp: {}, \
             reputation_discount: {}, given_item_modifier: &[{}], merchant_predicate: {}, \
             double_trade_price_enchantments: {} }},",
            trade.name,
            cost_text(&trade.wants),
            optional(trade.additional_wants.as_ref().map(cost_text)),
            trade.gives.item,
            trade.gives.count,
            components_text(&trade.gives.components),
            int_text(&trade.max_uses),
            int_text(&trade.xp),
            float(trade.reputation_discount),
            modifiers_text(&trade.given_item_modifier),
            optional(trade.merchant_predicate.as_ref().map(|variants| format!(
                "MerchantPredicate {{ villager_variants: &{variants:?} }}"
            ))),
            optional(
                trade
                    .double_trade_price_enchantments
                    .as_ref()
                    .map(named_text)
            )
        )?;
    }
    writeln!(body, "];\n")?;
    writeln!(
        body,
        "/// The trade sets, by name, each naming its trades by their numbers in \
         `VILLAGER_TRADES`."
    )?;
    writeln!(
        body,
        "pub static TRADE_SETS: [TradeSet; {}] = [",
        compiled.sets.len()
    )?;
    for set in &compiled.sets {
        let trades: Vec<String> = set.trades.iter().map(usize::to_string).collect();
        writeln!(
            body,
            "    TradeSet {{ name: {:?}, amount: {}, allow_duplicates: {}, random_sequence: {}, \
             trades: &[{}] }},",
            set.name,
            int_text(&set.amount),
            set.allow_duplicates,
            optional(set.random_sequence.as_ref().map(|name| format!("{name:?}"))),
            trades.join(", ")
        )?;
    }
    writeln!(body, "];")?;

    let used: Vec<&str> = TYPES
        .into_iter()
        .filter(|name| {
            body.contains(&format!("{name} {{"))
                || body.contains(&format!("{name}::"))
                || body.contains(&format!("[{name};"))
        })
        .collect();
    Ok(format!(
        "use crate::trading::{{{}}};\n\n{body}",
        used.join(", ")
    ))
}

fn optional(text: Option<String>) -> String {
    text.map_or_else(|| "None".to_owned(), |text| format!("Some({text})"))
}

fn float(bits: u32) -> String {
    format!("f32::from_bits({bits:#010x})")
}

fn int_text(provider: &IntProvider) -> String {
    match provider {
        IntProvider::Constant(value) => format!("ContextIntProvider::Constant({value})"),
        IntProvider::Uniform { min, max } => {
            format!("ContextIntProvider::Uniform {{ min: {min}, max: {max} }}")
        }
        IntProvider::Binomial { n, p } => {
            format!(
                "ContextIntProvider::Binomial {{ n: {n}, p: {} }}",
                float(*p)
            )
        }
        IntProvider::Add(inputs) => {
            let inputs: Vec<String> = inputs.iter().map(int_text).collect();
            format!("ContextIntProvider::Add(&[{}])", inputs.join(", "))
        }
    }
}

fn named_text(set: &NamedSet) -> String {
    match set {
        NamedSet::Tag(tag) => format!("NamedSet::Tag({tag:?})"),
        NamedSet::Listed(names) => format!("NamedSet::Listed(&{names:?})"),
    }
}

fn components_text(components: &[Component]) -> String {
    components
        .iter()
        .map(|component| match component {
            Component::PotionContents { potion } => {
                format!("TradeComponent::PotionContents {{ potion: {potion:?} }}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn cost_text(cost: &Cost) -> String {
    format!(
        "TradeCost {{ item: {}, count: {}, components: &[{}] }}",
        cost.item,
        int_text(&cost.count),
        components_text(&cost.components)
    )
}

fn enchantment_predicates_text(predicates: &[EnchantmentPredicate]) -> String {
    predicates
        .iter()
        .map(|predicate| {
            format!(
                "EnchantmentPredicate {{ enchantments: {}, min_level: {:?}, max_level: {:?} }}",
                optional(predicate.enchantments.as_ref().map(named_text)),
                predicate.min_level,
                predicate.max_level
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn filter_text(filter: &ItemFilter) -> String {
    let items = optional(filter.items.as_ref().map(|items| match items {
        ItemSet::Tag(tag) => format!("ItemSet::Tag({tag:?})"),
        ItemSet::Listed(ids) => format!("ItemSet::Listed(&{ids:?})"),
    }));
    let predicates: Vec<String> = filter
        .predicates
        .iter()
        .map(|predicate| match predicate {
            SubPredicate::Enchantments(listed) => format!(
                "ItemSubPredicate::Enchantments(&[{}])",
                enchantment_predicates_text(listed)
            ),
            SubPredicate::StoredEnchantments(listed) => format!(
                "ItemSubPredicate::StoredEnchantments(&[{}])",
                enchantment_predicates_text(listed)
            ),
            SubPredicate::DyedColor => "ItemSubPredicate::DyedColor".to_owned(),
            SubPredicate::MapId => "ItemSubPredicate::MapId".to_owned(),
        })
        .collect();
    format!(
        "ItemFilter {{ items: {items}, predicates: &[{}] }}",
        predicates.join(", ")
    )
}

fn modifiers_text(modifiers: &[Modifier]) -> String {
    modifiers
        .iter()
        .map(modifier_text)
        .collect::<Vec<_>>()
        .join(", ")
}

fn modifier_text(modifier: &Modifier) -> String {
    let options = |options: &Option<NamedSet>| optional(options.as_ref().map(named_text));
    match modifier {
        Modifier::EnchantRandomly {
            options: set,
            only_compatible,
            include_additional_cost_component,
        } => format!(
            "TradeModifier::EnchantRandomly {{ options: {}, only_compatible: {only_compatible}, \
             include_additional_cost_component: {include_additional_cost_component} }}",
            options(set)
        ),
        Modifier::EnchantWithLevels {
            levels,
            options: set,
            include_additional_cost_component,
        } => format!(
            "TradeModifier::EnchantWithLevels {{ levels: {}, options: {}, \
             include_additional_cost_component: {include_additional_cost_component} }}",
            int_text(levels),
            options(set)
        ),
        Modifier::Filtered {
            item_filter,
            on_pass,
            on_fail,
        } => format!(
            "TradeModifier::Filtered {{ item_filter: {}, on_pass: &[{}], on_fail: &[{}] }}",
            filter_text(item_filter),
            modifiers_text(on_pass),
            modifiers_text(on_fail)
        ),
        Modifier::Discard => "TradeModifier::Discard".to_owned(),
        Modifier::SetRandomDyes { number_of_dyes } => format!(
            "TradeModifier::SetRandomDyes {{ number_of_dyes: {} }}",
            int_text(number_of_dyes)
        ),
        Modifier::SetRandomPotion { options: set } => format!(
            "TradeModifier::SetRandomPotion {{ options: {} }}",
            options(set)
        ),
        Modifier::SetStewEffect { effects } => {
            let effects: Vec<String> = effects
                .iter()
                .map(|entry| {
                    format!(
                        "StewEffect {{ effect: {:?}, duration: {} }}",
                        entry.effect,
                        int_text(&entry.duration)
                    )
                })
                .collect();
            format!(
                "TradeModifier::SetStewEffect {{ effects: &[{}] }}",
                effects.join(", ")
            )
        }
        Modifier::SetPotion { potion } => {
            format!("TradeModifier::SetPotion {{ potion: {potion:?} }}")
        }
        Modifier::SetCount { count, add } => format!(
            "TradeModifier::SetCount {{ count: {}, add: {add} }}",
            int_text(count)
        ),
        Modifier::ExplorationMap {
            destination,
            decoration,
            zoom,
            search_radius,
            skip_existing_chunks,
        } => format!(
            "TradeModifier::ExplorationMap {{ destination: {}, decoration: {decoration:?}, \
             zoom: {zoom}, search_radius: {search_radius}, \
             skip_existing_chunks: {skip_existing_chunks} }}",
            named_text(destination)
        ),
    }
}

#[cfg(test)]
#[path = "trades_tests.rs"]
mod tests;
