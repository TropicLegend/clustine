//! Compiles the block loot tables of the game's data into the compact table that
//! `clustine-data` reads (`docs/groundwork/n4-brief.md`, sections 8.2 and 8.3).
//!
//! The tables are `data/minecraft/loot_table/blocks/*.json` in 26.3's form: conditions
//! under `condition` (an object, or a string naming a file of `data/minecraft/predicate`),
//! item modifiers under `modifier` (an object or a list) on an entry, a pool or the
//! whole table. Only what the census of the brief found is understood; anything else
//! (another kind of entry, condition or modifier, a weight, a number that is not a
//! constant where the game's data has constants, a key no reader here knows) fails the
//! run, so that the day a jar adds one is noticed and not dropped in silence.
//!
//! Conditions and modifiers are numbered once each, distinct ones in the order they
//! are first met, and a table names them by number.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;
use crate::model::Block;

/// What a block's state has to be: the block, and each named property's value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StatePredicate {
    pub block: usize,
    pub properties: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Condition {
    SurvivesExplosion,
    MatchBlock(StatePredicate),
    LocationCheck {
        offset: [i32; 3],
        block: StatePredicate,
    },
    AnyOf(Vec<usize>),
    AllOf(Vec<usize>),
    Inverted(usize),
    /// The chances as float bits.
    TableBonus {
        enchantment: String,
        chances: Vec<u32>,
    },
    /// The chance as float bits.
    RandomChance(u32),
    /// `entity_properties` with an empty predicate: there has to be such an entity.
    EntityProperties {
        entity: String,
    },
    /// `match_tool` whose tool has to be one of these items (an item, or a tag with
    /// `#`).
    MatchToolItems(String),
    /// `match_tool` whose tool has to have the enchantment at a level of at least
    /// `min_level`.
    MatchToolEnchantment {
        enchantment: String,
        min_level: i32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BonusFormula {
    OreDrops,
    UniformBonusCount {
        bonus_multiplier: i32,
    },
    /// `probability` as float bits.
    BinomialWithBonusCount {
        extra: i32,
        probability: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModifierKind {
    SetCount {
        count: IntProvider,
        add: bool,
    },
    ExplosionDecay,
    ApplyBonus {
        enchantment: String,
        formula: BonusFormula,
    },
    LimitCount {
        min: Option<i32>,
        max: Option<i32>,
    },
    /// Copies components of the block entity, which Clustine's stacks do not have.
    CopyComponents,
    CopyState {
        block: usize,
        properties: Vec<String>,
    },
}

/// An item modifier and the condition it is applied under.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Modifier {
    pub kind: ModifierKind,
    pub condition: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item {
        item: usize,
        condition: Option<usize>,
        modifiers: Vec<usize>,
    },
    Alternatives {
        condition: Option<usize>,
        children: Vec<Entry>,
    },
    Dynamic {
        name: String,
        condition: Option<usize>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    pub rolls: i32,
    pub condition: Option<usize>,
    pub entries: Vec<Entry>,
    pub modifiers: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// The table's name, such as `minecraft:blocks/oak_leaves`.
    pub name: String,
    pub pools: Vec<Pool>,
    pub modifiers: Vec<usize>,
}

/// The block loot tables, sorted by name, with the conditions and modifiers they name.
#[derive(Debug, Default)]
pub struct Compiled {
    pub conditions: Vec<Condition>,
    pub modifiers: Vec<Modifier>,
    pub tables: Vec<Table>,
    condition_numbers: BTreeMap<Condition, usize>,
    modifier_numbers: BTreeMap<Modifier, usize>,
}

/// Where the predicate files are read from: a file's text by its path below
/// `data/minecraft/predicate`, without `.json`.
pub trait Predicates {
    fn read(&self, path: &str) -> Result<String>;
}

/// The predicate files of a directory.
pub struct PredicateDirectory<'a>(pub &'a Path);

impl Predicates for PredicateDirectory<'_> {
    fn read(&self, path: &str) -> Result<String> {
        ensure!(
            path.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_/".contains(c)),
            "{path:?} is no predicate's name"
        );
        let file = self.0.join(format!("{path}.json"));
        fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))
    }
}

/// The context the tables are read in: the blocks and the items by name.
pub struct Names<'a> {
    pub blocks: &'a [Block],
    pub items: &'a [String],
}

impl Names<'_> {
    fn block(&self, name: &str) -> Result<usize> {
        self.blocks
            .iter()
            .position(|block| block.name == name)
            .with_context(|| format!("there is no block {name}"))
    }

    fn item(&self, name: &str) -> Result<usize> {
        self.items
            .iter()
            .position(|item| item == name)
            .with_context(|| format!("there is no item {name}"))
    }
}

/// Reads every file of `directory` (the block loot tables) with the predicates of
/// `predicates`.
pub fn load(directory: &Path, predicates: &dyn Predicates, names: &Names) -> Result<Compiled> {
    let mut files = Vec::new();
    for entry in
        fs::read_dir(directory).with_context(|| format!("listing {}", directory.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_str().context("a file name is not UTF-8")?;
        ensure!(
            entry.file_type()?.is_file(),
            "{file_name} in the block loot tables is not a file"
        );
        let stem = file_name
            .strip_suffix(".json")
            .with_context(|| format!("{file_name} is not a JSON file"))?;
        files.push(stem.to_owned());
    }
    files.sort();
    let mut compiled = Compiled::default();
    for stem in files {
        let path = directory.join(format!("{stem}.json"));
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        compiled
            .add(
                &format!("minecraft:blocks/{stem}"),
                &text,
                predicates,
                names,
            )
            .with_context(|| format!("reading the loot table {}", path.display()))?;
    }
    Ok(compiled)
}

/// Fails unless the object `value` has no keys but `allowed`.
fn only(value: &Json, allowed: &[&str]) -> Result<()> {
    for (key, _) in value.members()? {
        ensure!(
            allowed.contains(&key.as_str()),
            "{key:?} is not understood here (only {allowed:?})"
        );
    }
    Ok(())
}

/// The `type` of an object, without `minecraft:`.
fn kind(value: &Json) -> Result<&str> {
    let kind = value.member("type")?.string()?;
    kind.strip_prefix("minecraft:")
        .with_context(|| format!("the type {kind} is not the game's"))
}

/// The value of `value` read as a list: a list, or one value standing alone.
fn one_or_many(value: &Json) -> Vec<&Json> {
    match value {
        Json::Array(values) => values.iter().collect(),
        single => vec![single],
    }
}

fn float_bits(value: &Json) -> Result<u32> {
    Ok(value.float()?.to_bits())
}

impl Compiled {
    /// Adds the table called `name` with the text `text`.
    pub fn add(
        &mut self,
        name: &str,
        text: &str,
        predicates: &dyn Predicates,
        names: &Names,
    ) -> Result<()> {
        let json = Json::parse(text)?;
        only(&json, &["type", "pools", "modifier", "random_sequence"])?;
        ensure!(
            kind(&json)? == "block",
            "a block's table is of the type block"
        );
        let sequence = json.member("random_sequence")?.string()?;
        ensure!(
            sequence == name,
            "the random sequence {sequence} is not the table's name"
        );
        let mut pools = Vec::new();
        if let Some(listed) = json.get("pools") {
            for (index, pool) in listed.array()?.iter().enumerate() {
                pools.push(
                    self.pool(pool, predicates, names)
                        .with_context(|| format!("in pool {index}"))?,
                );
            }
        }
        let modifiers = self.modifiers(json.get("modifier"), predicates, names)?;
        self.tables.push(Table {
            name: name.to_owned(),
            pools,
            modifiers,
        });
        Ok(())
    }

    fn pool(&mut self, pool: &Json, predicates: &dyn Predicates, names: &Names) -> Result<Pool> {
        only(pool, &["rolls", "entries", "condition", "modifier"])?;
        let rolls = pool
            .member("rolls")?
            .integer()
            .context("the rolls of a pool are a constant")?;
        let condition = self.optional_condition(pool.get("condition"), predicates, names)?;
        let mut entries = Vec::new();
        for entry in pool.member("entries")?.array()? {
            entries.push(self.entry(entry, predicates, names)?);
        }
        let modifiers = self.modifiers(pool.get("modifier"), predicates, names)?;
        Ok(Pool {
            rolls,
            condition,
            entries,
            modifiers,
        })
    }

    fn entry(&mut self, entry: &Json, predicates: &dyn Predicates, names: &Names) -> Result<Entry> {
        let condition = self.optional_condition(entry.get("condition"), predicates, names)?;
        Ok(match kind(entry)? {
            "item" => {
                only(entry, &["type", "name", "condition", "modifier"])?;
                Entry::Item {
                    item: names.item(entry.member("name")?.string()?)?,
                    condition,
                    modifiers: self.modifiers(entry.get("modifier"), predicates, names)?,
                }
            }
            "alternatives" => {
                only(entry, &["type", "children", "condition"])?;
                let mut children = Vec::new();
                for child in entry.member("children")?.array()? {
                    children.push(self.entry(child, predicates, names)?);
                }
                Entry::Alternatives {
                    condition,
                    children,
                }
            }
            "dynamic" => {
                only(entry, &["type", "name", "condition"])?;
                Entry::Dynamic {
                    name: entry.member("name")?.string()?.to_owned(),
                    condition,
                }
            }
            other => bail!("an entry of the type {other} is not understood"),
        })
    }

    fn optional_condition(
        &mut self,
        value: Option<&Json>,
        predicates: &dyn Predicates,
        names: &Names,
    ) -> Result<Option<usize>> {
        value
            .map(|value| self.condition(value, predicates, names))
            .transpose()
    }

    fn condition(
        &mut self,
        value: &Json,
        predicates: &dyn Predicates,
        names: &Names,
    ) -> Result<usize> {
        let condition = match value {
            Json::String(reference) => {
                let path = reference
                    .strip_prefix("minecraft:")
                    .with_context(|| format!("the predicate {reference} is not the game's"))?;
                let text = predicates.read(path)?;
                let json = Json::parse(&text)
                    .with_context(|| format!("reading the predicate {reference}"))?;
                return self
                    .condition(&json, predicates, names)
                    .with_context(|| format!("in the predicate {reference}"));
            }
            _ => self.condition_object(value, predicates, names)?,
        };
        Ok(self.number_condition(condition))
    }

    fn condition_object(
        &mut self,
        value: &Json,
        predicates: &dyn Predicates,
        names: &Names,
    ) -> Result<Condition> {
        Ok(match kind(value)? {
            "survives_explosion" => {
                only(value, &["type"])?;
                Condition::SurvivesExplosion
            }
            "match_block" => {
                only(value, &["type", "blocks", "state"])?;
                Condition::MatchBlock(state_predicate(value, names)?)
            }
            "location_check" => {
                only(
                    value,
                    &["type", "offsetX", "offsetY", "offsetZ", "predicate"],
                )?;
                let offset =
                    |key: &str| -> Result<i32> { value.get(key).map_or(Ok(0), Json::integer) };
                let predicate = value.member("predicate")?;
                only(predicate, &["block"])?;
                Condition::LocationCheck {
                    offset: [offset("offsetX")?, offset("offsetY")?, offset("offsetZ")?],
                    block: state_predicate(predicate.member("block")?, names)?,
                }
            }
            "any_of" | "all_of" => {
                only(value, &["type", "terms"])?;
                let mut terms = Vec::new();
                for term in value.member("terms")?.array()? {
                    terms.push(self.condition(term, predicates, names)?);
                }
                if kind(value)? == "any_of" {
                    Condition::AnyOf(terms)
                } else {
                    Condition::AllOf(terms)
                }
            }
            "inverted" => {
                only(value, &["type", "term"])?;
                Condition::Inverted(self.condition(value.member("term")?, predicates, names)?)
            }
            "table_bonus" => {
                only(value, &["type", "enchantment", "chances"])?;
                let chances = value
                    .member("chances")?
                    .array()?
                    .iter()
                    .map(float_bits)
                    .collect::<Result<Vec<_>>>()?;
                ensure!(!chances.is_empty(), "a table bonus without chances");
                Condition::TableBonus {
                    enchantment: value.member("enchantment")?.string()?.to_owned(),
                    chances,
                }
            }
            "random_chance" => {
                only(value, &["type", "chance"])?;
                Condition::RandomChance(
                    float_bits(value.member("chance")?).context("a chance is a constant")?,
                )
            }
            "entity_properties" => {
                only(value, &["type", "entity", "predicate"])?;
                ensure!(
                    value.member("predicate")?.members()?.is_empty(),
                    "an entity predicate that is not empty is not understood"
                );
                Condition::EntityProperties {
                    entity: value.member("entity")?.string()?.to_owned(),
                }
            }
            "match_tool" => {
                only(value, &["type", "predicate"])?;
                tool_predicate(value.member("predicate")?)?
            }
            other => bail!("a condition of the type {other} is not understood"),
        })
    }

    fn number_condition(&mut self, condition: Condition) -> usize {
        if let Some(&number) = self.condition_numbers.get(&condition) {
            return number;
        }
        let number = self.conditions.len();
        self.condition_numbers.insert(condition.clone(), number);
        self.conditions.push(condition);
        number
    }

    fn modifiers(
        &mut self,
        value: Option<&Json>,
        predicates: &dyn Predicates,
        names: &Names,
    ) -> Result<Vec<usize>> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let mut numbers = Vec::new();
        for modifier in one_or_many(value) {
            let kind = self.modifier_kind(modifier, names)?;
            let condition =
                self.optional_condition(modifier.get("condition"), predicates, names)?;
            let modifier = Modifier { kind, condition };
            let number = match self.modifier_numbers.get(&modifier) {
                Some(&number) => number,
                None => {
                    let number = self.modifiers.len();
                    self.modifier_numbers.insert(modifier.clone(), number);
                    self.modifiers.push(modifier);
                    number
                }
            };
            numbers.push(number);
        }
        Ok(numbers)
    }

    fn modifier_kind(&mut self, value: &Json, names: &Names) -> Result<ModifierKind> {
        Ok(match kind(value)? {
            "set_count" => {
                only(value, &["type", "count", "add", "condition"])?;
                ModifierKind::SetCount {
                    count: int_provider(value.member("count")?)?,
                    add: value.get("add").map_or(Ok(false), Json::boolean)?,
                }
            }
            "explosion_decay" => {
                only(value, &["type", "condition"])?;
                ModifierKind::ExplosionDecay
            }
            "apply_bonus" => {
                only(
                    value,
                    &["type", "enchantment", "formula", "parameters", "condition"],
                )?;
                let formula = value.member("formula")?.string()?;
                let parameters = value.get("parameters");
                let formula = match formula {
                    "minecraft:ore_drops" => {
                        ensure!(parameters.is_none(), "ore drops take no parameters");
                        BonusFormula::OreDrops
                    }
                    "minecraft:uniform_bonus_count" => {
                        let parameters =
                            parameters.context("a uniform bonus without parameters")?;
                        only(parameters, &["bonusMultiplier"])?;
                        BonusFormula::UniformBonusCount {
                            bonus_multiplier: parameters.member("bonusMultiplier")?.integer()?,
                        }
                    }
                    "minecraft:binomial_with_bonus_count" => {
                        let parameters =
                            parameters.context("a binomial bonus without parameters")?;
                        only(parameters, &["extra", "probability"])?;
                        BonusFormula::BinomialWithBonusCount {
                            extra: parameters.member("extra")?.integer()?,
                            probability: float_bits(parameters.member("probability")?)?,
                        }
                    }
                    other => bail!("the bonus formula {other} is not understood"),
                };
                ModifierKind::ApplyBonus {
                    enchantment: value.member("enchantment")?.string()?.to_owned(),
                    formula,
                }
            }
            "limit_count" => {
                only(value, &["type", "limit", "condition"])?;
                let limit = value.member("limit")?;
                only(limit, &["min", "max"])?;
                ModifierKind::LimitCount {
                    min: limit.get("min").map(Json::integer).transpose()?,
                    max: limit.get("max").map(Json::integer).transpose()?,
                }
            }
            "copy_components" => {
                only(value, &["type", "source", "include", "condition"])?;
                ensure!(
                    value.member("source")?.string()? == "block_entity",
                    "components copied from elsewhere than the block entity are not understood"
                );
                for component in value.member("include")?.array()? {
                    component.string()?;
                }
                ModifierKind::CopyComponents
            }
            "copy_state" => {
                only(value, &["type", "block", "properties", "condition"])?;
                let block = names.block(value.member("block")?.string()?)?;
                let mut properties = Vec::new();
                for property in value.member("properties")?.array()? {
                    let property = property.string()?;
                    ensure!(
                        names.blocks[block]
                            .properties
                            .iter()
                            .any(|known| known.name == property),
                        "{} has no property {property}",
                        names.blocks[block].name
                    );
                    properties.push(property.to_owned());
                }
                ModifierKind::CopyState { block, properties }
            }
            other => bail!("a modifier of the type {other} is not understood"),
        })
    }
}

/// The block and properties of a `match_block` or of a location's block predicate.
fn state_predicate(value: &Json, names: &Names) -> Result<StatePredicate> {
    let name = value.member("blocks")?.string()?;
    ensure!(
        !name.starts_with('#'),
        "a block tag in a predicate is not understood"
    );
    let block = names.block(name)?;
    let mut properties = Vec::new();
    if let Some(state) = value.get("state") {
        for (property, wanted) in state.members()? {
            let wanted = wanted
                .string()
                .with_context(|| format!("the value of {property} is not one value"))?;
            let known = names.blocks[block]
                .properties
                .iter()
                .find(|known| known.name == *property)
                .with_context(|| format!("{name} has no property {property}"))?;
            ensure!(
                known.values.iter().any(|value| value == wanted),
                "{wanted} is no value of {property} of {name}"
            );
            properties.push((property.clone(), wanted.to_owned()));
        }
    }
    Ok(StatePredicate { block, properties })
}

/// The predicate of a `match_tool`: items, or one enchantment with a lowest level.
fn tool_predicate(predicate: &Json) -> Result<Condition> {
    if let Some(items) = predicate.get("items") {
        only(predicate, &["items"])?;
        return Ok(Condition::MatchToolItems(items.string()?.to_owned()));
    }
    only(predicate, &["predicates"])?;
    let predicates = predicate.member("predicates")?;
    only(predicates, &["minecraft:enchantments"])?;
    let [enchantment] = predicates.member("minecraft:enchantments")?.array()? else {
        bail!("a tool predicate of other than one enchantment is not understood");
    };
    only(enchantment, &["enchantments", "levels"])?;
    let levels = enchantment.member("levels")?;
    only(levels, &["min"])?;
    Ok(Condition::MatchToolEnchantment {
        enchantment: enchantment.member("enchantments")?.string()?.to_owned(),
        min_level: levels.member("min")?.integer()?,
    })
}

/// A count: a constant, `uniform` or `binomial`, each of constants.
fn int_provider(value: &Json) -> Result<IntProvider> {
    if let Json::Number(_) = value {
        return Ok(IntProvider::Constant(value.integer()?));
    }
    Ok(match kind(value)? {
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
        other => bail!("a count of the type {other} is not understood"),
    })
}

/// `loot_tables.rs`: the conditions, the modifiers and the tables, and for each block
/// the number of its table. `block_tables` are the names of the blocks' tables by
/// block id.
pub fn emit(compiled: &Compiled, block_tables: &[Option<&str>]) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "use crate::loot::{{BonusFormula, IntProvider, LootCondition, LootEntry, LootModifier, \
         LootPool, LootTable, StatePredicate}};\n"
    )?;
    writeln!(
        out,
        "/// The distinct conditions of the block loot tables, which they name by number."
    )?;
    writeln!(
        out,
        "pub static CONDITIONS: [LootCondition; {}] = [",
        compiled.conditions.len()
    )?;
    for (number, condition) in compiled.conditions.iter().enumerate() {
        writeln!(out, "    {}, // {number}", condition_text(condition)?)?;
    }
    writeln!(out, "];\n")?;
    writeln!(
        out,
        "/// The distinct item modifiers of the block loot tables, which they name by number."
    )?;
    writeln!(
        out,
        "pub static MODIFIERS: [LootModifier; {}] = [",
        compiled.modifiers.len()
    )?;
    for (number, modifier) in compiled.modifiers.iter().enumerate() {
        writeln!(out, "    {}, // {number}", modifier_text(modifier)?)?;
    }
    writeln!(out, "];\n")?;
    writeln!(out, "/// The block loot tables, by name.")?;
    writeln!(
        out,
        "pub static TABLES: [LootTable; {}] = [",
        compiled.tables.len()
    )?;
    for table in &compiled.tables {
        let mut pools = Vec::new();
        for pool in &table.pools {
            let entries = pool
                .entries
                .iter()
                .map(entry_text)
                .collect::<Result<Vec<_>>>()?;
            pools.push(format!(
                "LootPool {{ rolls: {}, condition: {}, entries: &[{}], modifiers: &[{}] }}",
                pool.rolls,
                option(pool.condition)?,
                entries.join(", "),
                numbers(&pool.modifiers)?
            ));
        }
        writeln!(
            out,
            "    LootTable {{ name: {:?}, pools: &[{}], modifiers: &[{}] }},",
            table.name,
            pools.join(", "),
            numbers(&table.modifiers)?
        )?;
    }
    writeln!(out, "];\n")?;

    writeln!(
        out,
        "/// The number of each block's table in `TABLES`, by block id; `u16::MAX` for a \
         block that\n/// has none, or names a table the game does not have, which drops \
         nothing."
    )?;
    writeln!(
        out,
        "pub static BLOCK_TABLES: [u16; {}] = [",
        block_tables.len()
    )?;
    ensure!(
        compiled.tables.len() < usize::from(u16::MAX),
        "more loot tables than 16 bits can number"
    );
    let mut indices = Vec::new();
    for name in block_tables {
        let index = match name {
            Some(name) => compiled
                .tables
                .binary_search_by(|table| table.name.as_str().cmp(name))
                .unwrap_or(usize::from(u16::MAX)),
            None => usize::from(u16::MAX),
        };
        indices.push(index);
    }
    out.push_str(&crate::emit::wrapped(&indices, "    "));
    writeln!(out, "];")?;
    Ok(out)
}

fn small(number: usize) -> Result<u16> {
    u16::try_from(number)
        .ok()
        .filter(|&number| number < u16::MAX)
        .context("more conditions or modifiers than 16 bits can number")
}

fn option(number: Option<usize>) -> Result<String> {
    Ok(match number {
        Some(number) => format!("Some({})", small(number)?),
        None => "None".to_owned(),
    })
}

fn numbers(list: &[usize]) -> Result<String> {
    Ok(list
        .iter()
        .map(|&number| small(number).map(|number| number.to_string()))
        .collect::<Result<Vec<_>>>()?
        .join(", "))
}

fn float(bits: u32) -> String {
    format!("f32::from_bits({bits:#010x})")
}

fn state_text(predicate: &StatePredicate) -> Result<String> {
    let properties: Vec<String> = predicate
        .properties
        .iter()
        .map(|(name, value)| format!("({name:?}, {value:?})"))
        .collect();
    Ok(format!(
        "StatePredicate {{ block: {}, properties: &[{}] }}",
        small(predicate.block)?,
        properties.join(", ")
    ))
}

fn condition_text(condition: &Condition) -> Result<String> {
    Ok(match condition {
        Condition::SurvivesExplosion => "LootCondition::SurvivesExplosion".to_owned(),
        Condition::MatchBlock(predicate) => {
            format!("LootCondition::MatchBlock({})", state_text(predicate)?)
        }
        Condition::LocationCheck { offset, block } => format!(
            "LootCondition::LocationCheck {{ offset: [{}, {}, {}], block: {} }}",
            offset[0],
            offset[1],
            offset[2],
            state_text(block)?
        ),
        Condition::AnyOf(terms) => format!("LootCondition::AnyOf(&[{}])", numbers(terms)?),
        Condition::AllOf(terms) => format!("LootCondition::AllOf(&[{}])", numbers(terms)?),
        Condition::Inverted(term) => format!("LootCondition::Inverted({})", small(*term)?),
        Condition::TableBonus {
            enchantment,
            chances,
        } => {
            let chances: Vec<String> = chances.iter().map(|&bits| float(bits)).collect();
            format!(
                "LootCondition::TableBonus {{ enchantment: {enchantment:?}, chances: &[{}] }}",
                chances.join(", ")
            )
        }
        Condition::RandomChance(bits) => {
            format!("LootCondition::RandomChance {{ chance: {} }}", float(*bits))
        }
        Condition::EntityProperties { entity } => {
            format!("LootCondition::EntityProperties {{ entity: {entity:?} }}")
        }
        Condition::MatchToolItems(items) => {
            format!("LootCondition::MatchToolItems {{ items: {items:?} }}")
        }
        Condition::MatchToolEnchantment {
            enchantment,
            min_level,
        } => format!(
            "LootCondition::MatchToolEnchantment {{ enchantment: {enchantment:?}, \
             min_level: {min_level} }}"
        ),
    })
}

fn modifier_text(modifier: &Modifier) -> Result<String> {
    let condition = option(modifier.condition)?;
    Ok(match &modifier.kind {
        ModifierKind::SetCount { count, add } => {
            let count = match count {
                IntProvider::Constant(value) => format!("IntProvider::Constant({value})"),
                IntProvider::Uniform { min, max } => {
                    format!("IntProvider::Uniform {{ min: {min}, max: {max} }}")
                }
                IntProvider::Binomial { n, p } => {
                    format!("IntProvider::Binomial {{ n: {n}, p: {} }}", float(*p))
                }
            };
            format!(
                "LootModifier::SetCount {{ count: {count}, add: {add}, condition: {condition} }}"
            )
        }
        ModifierKind::ExplosionDecay => {
            format!("LootModifier::ExplosionDecay {{ condition: {condition} }}")
        }
        ModifierKind::ApplyBonus {
            enchantment,
            formula,
        } => {
            let formula = match formula {
                BonusFormula::OreDrops => "BonusFormula::OreDrops".to_owned(),
                BonusFormula::UniformBonusCount { bonus_multiplier } => format!(
                    "BonusFormula::UniformBonusCount {{ bonus_multiplier: {bonus_multiplier} }}"
                ),
                BonusFormula::BinomialWithBonusCount { extra, probability } => format!(
                    "BonusFormula::BinomialWithBonusCount {{ extra: {extra}, probability: {} }}",
                    float(*probability)
                ),
            };
            format!(
                "LootModifier::ApplyBonus {{ enchantment: {enchantment:?}, formula: {formula}, \
                 condition: {condition} }}"
            )
        }
        ModifierKind::LimitCount { min, max } => format!(
            "LootModifier::LimitCount {{ min: {min:?}, max: {max:?}, condition: {condition} }}"
        ),
        ModifierKind::CopyComponents => {
            format!("LootModifier::CopyComponents {{ condition: {condition} }}")
        }
        ModifierKind::CopyState { block, properties } => format!(
            "LootModifier::CopyState {{ block: {}, properties: &{properties:?}, \
             condition: {condition} }}",
            small(*block)?
        ),
    })
}

fn entry_text(entry: &Entry) -> Result<String> {
    Ok(match entry {
        Entry::Item {
            item,
            condition,
            modifiers,
        } => format!(
            "LootEntry::Item {{ item: {}, condition: {}, modifiers: &[{}] }}",
            small(*item)?,
            option(*condition)?,
            numbers(modifiers)?
        ),
        Entry::Alternatives {
            condition,
            children,
        } => {
            let children = children
                .iter()
                .map(entry_text)
                .collect::<Result<Vec<_>>>()?;
            format!(
                "LootEntry::Alternatives {{ condition: {}, children: &[{}] }}",
                option(*condition)?,
                children.join(", ")
            )
        }
        Entry::Dynamic { name, condition } => format!(
            "LootEntry::Dynamic {{ name: {name:?}, condition: {} }}",
            option(*condition)?
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Property;

    struct Files;

    impl Predicates for Files {
        fn read(&self, path: &str) -> Result<String> {
            Ok(match path {
                "tool/can_shear" => {
                    r#"{"type": "minecraft:match_tool", "predicate": {"items": "minecraft:shears"}}"#
                }
                "tool/can_silk_touch" => {
                    r#"{"type": "minecraft:match_tool", "predicate": {"predicates":
                    {"minecraft:enchantments": [{"enchantments": "minecraft:silk_touch",
                    "levels": {"min": 1}}]}}}"#
                }
                other => bail!("no predicate {other}"),
            }
            .to_owned())
        }
    }

    fn blocks() -> Vec<Block> {
        vec![
            Block {
                name: "minecraft:oak_leaves".to_owned(),
                first_state: 0,
                last_state: 0,
                default_state: 0,
                properties: vec![],
            },
            Block {
                name: "minecraft:wheat".to_owned(),
                first_state: 1,
                last_state: 8,
                default_state: 1,
                properties: vec![Property {
                    name: "age".to_owned(),
                    values: (0..8).map(|age| age.to_string()).collect(),
                }],
            },
        ]
    }

    fn items() -> Vec<String> {
        [
            "minecraft:air",
            "minecraft:oak_leaves",
            "minecraft:stick",
            "minecraft:wheat",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    const LEAVES: &str = r#"{
      "type": "minecraft:block",
      "pools": [
        {"rolls": 1, "entries": [{"type": "minecraft:alternatives", "children": [
          {"type": "minecraft:item", "name": "minecraft:oak_leaves", "condition":
            {"type": "minecraft:any_of", "terms": ["minecraft:tool/can_shear",
              "minecraft:tool/can_silk_touch"]}},
          {"type": "minecraft:item", "name": "minecraft:stick", "condition":
            {"type": "minecraft:table_bonus", "enchantment": "minecraft:fortune",
             "chances": [0.02, 0.022222223]},
           "modifier": [{"type": "minecraft:set_count", "count":
             {"type": "minecraft:uniform", "min": 1, "max": 2}},
             {"type": "minecraft:explosion_decay"}]}
        ]}]},
        {"rolls": 1, "condition": {"type": "minecraft:inverted", "term":
          {"type": "minecraft:any_of", "terms": ["minecraft:tool/can_shear",
            "minecraft:tool/can_silk_touch"]}},
         "entries": [{"type": "minecraft:item", "name": "minecraft:wheat", "modifier":
           {"type": "minecraft:set_count", "count": 2, "condition":
             {"type": "minecraft:match_block", "blocks": "minecraft:wheat",
              "state": {"age": "7"}}}}]}
      ],
      "random_sequence": "minecraft:blocks/oak_leaves"
    }"#;

    fn compile(text: &str) -> Result<Compiled> {
        let blocks = blocks();
        let items = items();
        let names = Names {
            blocks: &blocks,
            items: &items,
        };
        let mut compiled = Compiled::default();
        compiled.add("minecraft:blocks/oak_leaves", text, &Files, &names)?;
        Ok(compiled)
    }

    #[test]
    fn a_table_is_compiled_with_its_predicate_files_inlined_and_its_conditions_shared() {
        let compiled = compile(LEAVES).unwrap();
        let table = &compiled.tables[0];
        assert_eq!(table.pools.len(), 2);
        let Entry::Alternatives { children, .. } = &table.pools[0].entries[0] else {
            panic!("the first entry is the alternatives");
        };
        let Entry::Item {
            item, condition, ..
        } = &children[0]
        else {
            panic!("the first child is an item");
        };
        assert_eq!(*item, 1);
        let Condition::AnyOf(terms) = &compiled.conditions[condition.unwrap()] else {
            panic!("the leaves drop under any of two");
        };
        assert_eq!(
            compiled.conditions[terms[0]],
            Condition::MatchToolItems("minecraft:shears".to_owned())
        );
        assert_eq!(
            compiled.conditions[terms[1]],
            Condition::MatchToolEnchantment {
                enchantment: "minecraft:silk_touch".to_owned(),
                min_level: 1
            }
        );
        // The second pool's inverted condition names the same "any of" by its number.
        assert_eq!(
            compiled.conditions[table.pools[1].condition.unwrap()],
            Condition::Inverted(condition.unwrap())
        );
        let Entry::Item { modifiers, .. } = &children[1] else {
            panic!("the second child is an item");
        };
        assert_eq!(
            compiled.modifiers[modifiers[0]].kind,
            ModifierKind::SetCount {
                count: IntProvider::Uniform { min: 1, max: 2 },
                add: false
            }
        );
        assert_eq!(
            compiled.modifiers[modifiers[1]].kind,
            ModifierKind::ExplosionDecay
        );
        let Entry::Item { modifiers, .. } = &table.pools[1].entries[0] else {
            panic!("an item");
        };
        let modifier = &compiled.modifiers[modifiers[0]];
        assert_eq!(
            compiled.conditions[modifier.condition.unwrap()],
            Condition::MatchBlock(StatePredicate {
                block: 1,
                properties: vec![("age".to_owned(), "7".to_owned())]
            })
        );
        let Condition::TableBonus { chances, .. } = &compiled.conditions[3] else {
            panic!("the table bonus comes fourth: {:?}", compiled.conditions);
        };
        assert_eq!(chances, &[0.02f32.to_bits(), 0.022222223f32.to_bits()]);

        let text = emit(&compiled, &[Some("minecraft:blocks/oak_leaves"), None]).unwrap();
        assert!(text.contains("LootCondition::Inverted(2)"), "{text}");
        assert!(text.contains("pub static BLOCK_TABLES: [u16; 2] = [\n    0, 65535,\n];"));
    }

    #[test]
    fn what_is_not_understood_is_refused() {
        let refused = [
            // A key nobody reads.
            LEAVES.replace(
                "\"rolls\": 1, \"condition\"",
                "\"rolls\": 1, \"bonus_rolls\": 1, \"condition\"",
            ),
            // Another kind of entry.
            LEAVES.replace("minecraft:alternatives", "minecraft:group"),
            // An unknown condition and an unknown modifier.
            LEAVES.replace("minecraft:inverted", "minecraft:weather_check"),
            LEAVES.replace("minecraft:explosion_decay", "minecraft:enchant_randomly"),
            // A rolls that is not a constant.
            LEAVES.replacen(
                "\"rolls\": 1",
                "\"rolls\": {\"type\": \"minecraft:uniform\", \"min\": 1, \"max\": 2}",
                1,
            ),
            // A property that the block lacks, a value it does not have.
            LEAVES.replace("{\"age\": \"7\"}", "{\"stage\": \"7\"}"),
            LEAVES.replace("{\"age\": \"7\"}", "{\"age\": \"9\"}"),
            // An item that is not one, a predicate file that is not there.
            LEAVES.replace("minecraft:stick", "minecraft:twig"),
            LEAVES.replacen("minecraft:tool/can_shear", "minecraft:tool/can_cut", 1),
            // Another random sequence.
            LEAVES.replace("blocks/oak_leaves\"\n", "blocks/birch_leaves\"\n"),
        ];
        for (index, text) in refused.iter().enumerate() {
            assert_ne!(text, LEAVES, "case {index} changed nothing");
            assert!(compile(text).is_err(), "case {index} was accepted");
        }
    }

    #[test]
    fn a_table_without_pools_and_counts_of_every_kind_are_read() {
        let compiled = compile(
            r#"{"type": "minecraft:block", "random_sequence": "minecraft:blocks/oak_leaves"}"#,
        )
        .unwrap();
        assert!(compiled.tables[0].pools.is_empty());

        let read = |text: &str| int_provider(&Json::parse(text).unwrap());
        assert_eq!(read("3").unwrap(), IntProvider::Constant(3));
        assert_eq!(
            read(r#"{"type": "minecraft:binomial", "n": 3, "p": 0.5714286}"#).unwrap(),
            IntProvider::Binomial {
                n: 3,
                p: 0.5714286f32.to_bits()
            }
        );
        assert!(read("2.5").is_err());
        assert!(read(r#"{"type": "minecraft:constant", "value": 2}"#).is_err());
    }

    /// Reads every block loot table and every item's components of the data
    /// generator's output that `cargo datagen` keeps in `target/datagen`; needs no Java.
    /// Run it with `cargo test -p clustine-datagen -- --ignored generated_data`.
    #[test]
    #[ignore = "needs the data generator's output in target/datagen"]
    fn every_table_and_every_item_of_the_generated_data_is_understood() {
        use crate::model;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let generated = root
            .join("target/datagen")
            .join(crate::jar::SERVER_JAR.version)
            .join("generated");
        let reports = generated.join("reports");
        let data = generated.join("data/minecraft");
        let registries = model::read_json(&reports.join("registries.json")).unwrap();
        let block_names = model::static_registry(&registries, "block").unwrap();
        let blocks = model::load_blocks(&reports, &block_names).unwrap();
        let items = model::static_registry(&registries, "item").unwrap();
        let names = Names {
            blocks: &blocks,
            items: &items,
        };
        let compiled = load(
            &data.join("loot_table/blocks"),
            &PredicateDirectory(&data.join("predicate")),
            &names,
        )
        .unwrap();
        assert_eq!(compiled.tables.len(), 1201);
        let by_name: Vec<String> = block_names
            .iter()
            .map(|name| name.replace("minecraft:", "minecraft:blocks/"))
            .collect();
        let tables: Vec<Option<&str>> = by_name.iter().map(|name| Some(name.as_str())).collect();
        let text = emit(&compiled, &tables).unwrap();
        println!(
            "{} conditions, {} modifiers, {} bytes",
            compiled.conditions.len(),
            compiled.modifiers.len(),
            text.len()
        );
        let mut stack_sizes = BTreeMap::new();
        for item in &items {
            let path = reports.join(format!(
                "minecraft/components/item/{}.json",
                item.strip_prefix("minecraft:").unwrap()
            ));
            let components = model::item_components_of(&model::read_json(&path).unwrap())
                .unwrap_or_else(|error| panic!("{item}: {error:#}"));
            *stack_sizes.entry(components.max_stack_size).or_insert(0) += 1;
        }
        assert_eq!(
            stack_sizes,
            BTreeMap::from([(1, 248), (16, 68), (64, 1342)])
        );
        let out = std::env::var_os("CLUSTINE_LOOT_TABLES_OUT");
        if let Some(out) = out {
            fs::write(out, text).unwrap();
        }
    }
}
