//! Compiles the loot tables of the game's data that Clustine reads into the compact table
//! that `clustine-data` reads: the blocks' (`docs/groundwork/n4-brief.md`, sections 8.2
//! and 8.3), the entities' and the tables they name (`docs/groundwork/n7-brief.md`,
//! section 1.7).
//!
//! The tables are `data/minecraft/loot_table/blocks/*.json` and
//! `data/minecraft/loot_table/entities/**/*.json` in 26.3's form, with the tables an
//! entity's names by a `loot_table` entry (a guardian's fish): conditions under
//! `condition` (an object, or a string naming a file of `data/minecraft/predicate`), item
//! modifiers under `modifier` (an object or a list) on an entry, a pool or the whole
//! table. Only what the census of the briefs found is understood; anything else (another
//! kind of entry, condition or modifier, a quality, a number that is not a constant where
//! the game's data has constants, a key no reader here knows) fails the run, so that the
//! day a jar adds one is noticed and not dropped in silence.
//!
//! Conditions and modifiers are numbered once each, distinct ones in the order they
//! are first met, and a table names them by number. What `furnace_smelt` makes of an
//! item is the game's smelting recipes', which are compiled beside the tables.

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
    /// `killed_by_player`: a player dealt the last damage within its memory.
    KilledByPlayer,
    /// `random_chance_with_enchanted_bonus`: the chances as float bits, the enchanted
    /// one `base + per_level · (level − 1)` (a `linear` level-based value; a constant is
    /// one of no step).
    RandomChanceWithEnchantedBonus {
        enchantment: String,
        unenchanted: u32,
        base: u32,
        per_level: u32,
    },
    /// `entity_properties` with a predicate that is not empty.
    EntityMatches {
        entity: String,
        predicate: EntityPredicate,
    },
    /// `damage_source_properties`.
    DamageSource(DamageSourcePredicate),
}

/// What an entity has to be (`EntityPredicate`), as far as the game's loot tables ask:
/// each part that is there has to hold.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntityPredicate {
    /// `minecraft:entity_type`: a type's name, or a tag's with `#`.
    pub kind: Option<String>,
    /// `minecraft:flags`' `is_on_fire`.
    pub on_fire: Option<bool>,
    /// `minecraft:flags`' `is_baby`.
    pub baby: Option<bool>,
    /// `minecraft:vehicle`'s `minecraft:entity_type`: the type of what it rides.
    pub vehicle: Option<String>,
    /// `minecraft:components`' `minecraft:sheep/color`: a dye's name.
    pub sheep_colour: Option<String>,
    /// `minecraft:components`' `minecraft:frog/variant`.
    pub frog_variant: Option<String>,
    /// `minecraft:components`' `minecraft:mooshroom/variant`: `red` or `brown`.
    pub mooshroom_variant: Option<String>,
    /// `minecraft:components`' `minecraft:chicken/variant`.
    pub chicken_variant: Option<String>,
    /// `minecraft:type_specific/sheep`'s `sheared`.
    pub sheared: Option<bool>,
    /// `minecraft:type_specific/cube_mob`'s `size`, as its least and greatest.
    pub cube_size: Option<(Option<i32>, Option<i32>)>,
    /// `minecraft:type_specific/raider`'s `is_captain`.
    pub captain: Option<bool>,
    /// `minecraft:equipment`'s `mainhand` with an enchantment of this tag (with `#`)
    /// or this name.
    pub main_hand_enchantment: Option<String>,
}

/// What a damage source has to be (`DamageSourcePredicate`).
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct DamageSourcePredicate {
    /// Damage type tags (with `#`), each with whether the type has to be in it.
    pub tags: Vec<(String, bool)>,
    /// `source_entity`: what caused it.
    pub source_entity: Option<EntityPredicate>,
    /// `direct_entity`: what dealt it.
    pub direct_entity: Option<EntityPredicate>,
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
    /// `enchanted_count_increase`: per level of the enchantment of the attacker's weapon,
    /// a uniform float from `min` to `max` (float bits) more, rounded, then held to
    /// `limit` where it is above 0.
    EnchantedCountIncrease {
        enchantment: String,
        min: u32,
        max: u32,
        limit: i32,
    },
    /// `furnace_smelt`: the item becomes what smelting it makes.
    FurnaceSmelt,
    /// `set_potion`: the potion of the stack, which Clustine's stacks do not carry.
    SetPotion {
        potion: String,
    },
    /// `set_ominous_bottle_amplifier`, which Clustine's stacks do not carry either; its
    /// count is drawn all the same.
    SetOminousBottleAmplifier {
        amplifier: IntProvider,
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
        weight: i32,
        condition: Option<usize>,
        modifiers: Vec<usize>,
    },
    /// Nothing, chosen by its weight among the others.
    Empty {
        weight: i32,
        condition: Option<usize>,
    },
    /// What the table of this name gives (`loot_table`).
    Table {
        name: String,
        weight: i32,
        condition: Option<usize>,
        modifiers: Vec<usize>,
    },
    /// The items of an item tag (with `#`): each as a choice of its own where
    /// `expand`, else every one of them.
    Tag {
        tag: String,
        expand: bool,
        weight: i32,
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
    pub rolls: IntProvider,
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

/// The loot tables, sorted by name, with the conditions and modifiers they name, and
/// what smelting makes of each item that smelts.
#[derive(Debug, Default)]
pub struct Compiled {
    pub conditions: Vec<Condition>,
    pub modifiers: Vec<Modifier>,
    pub tables: Vec<Table>,
    /// Items by id and what smelting each makes, ascending.
    pub smelting: Vec<(usize, usize)>,
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

/// The context the tables are read in: the blocks, the items and the entity types by
/// name.
pub struct Names<'a> {
    pub blocks: &'a [Block],
    pub items: &'a [String],
    pub entities: &'a [String],
}

impl Names<'_> {
    /// An entity type's name, or a tag's with `#`, checked where it is a type's.
    fn entity_kind(&self, name: &str) -> Result<String> {
        ensure!(
            name.starts_with('#') || self.entities.iter().any(|known| known == name),
            "there is no entity type {name}"
        );
        Ok(name.to_owned())
    }

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

/// The tables the farm animals' own code names (`docs/groundwork/n7-brief.md`, section
/// 4): a chicken's laying (`Chicken.aiStep`'s gift), a sheep's and a mooshroom's
/// shearing; each picks a nested table by the animal's variant or colour.
pub const ANIMALS: [&str; 3] = [
    "gameplay/chicken_lay",
    "shearing/mooshroom",
    "shearing/sheep",
];

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

/// The JSON files below `directory`, sub-directories included, as their paths from it
/// without `.json`, sorted.
fn tables_under(directory: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut pending = vec![(directory.to_path_buf(), String::new())];
    while let Some((at, prefix)) = pending.pop() {
        for entry in fs::read_dir(&at).with_context(|| format!("listing {}", at.display()))? {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_name = file_name.to_str().context("a file name is not UTF-8")?;
            if entry.file_type()?.is_dir() {
                pending.push((entry.path(), format!("{prefix}{file_name}/")));
                continue;
            }
            let stem = file_name
                .strip_suffix(".json")
                .with_context(|| format!("{file_name} is not a JSON file"))?;
            found.push(format!("{prefix}{stem}"));
        }
    }
    found.sort();
    Ok(found)
}

impl Compiled {
    /// Adds the entity tables of `loot_tables` (the directory `loot_table`), those of
    /// `entities` and below and the ones the animals' own code names ([`ANIMALS`]),
    /// then every table they name that is not there yet; and sorts the tables by name.
    pub fn add_entities(
        &mut self,
        loot_tables: &Path,
        predicates: &dyn Predicates,
        names: &Names,
    ) -> Result<()> {
        for stem in tables_under(&loot_tables.join("entities"))? {
            self.add_file(loot_tables, &format!("entities/{stem}"), predicates, names)?;
        }
        for path in ANIMALS {
            self.add_file(loot_tables, path, predicates, names)?;
        }
        loop {
            let missing: Vec<String> = self
                .named_tables()
                .into_iter()
                .filter(|name| !self.tables.iter().any(|table| table.name == *name))
                .collect();
            let Some(name) = missing.first() else {
                break;
            };
            let path = name
                .strip_prefix("minecraft:")
                .with_context(|| format!("the table {name} is not the game's"))?;
            self.add_file(loot_tables, path, predicates, names)?;
        }
        self.tables.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(())
    }

    fn add_file(
        &mut self,
        loot_tables: &Path,
        path: &str,
        predicates: &dyn Predicates,
        names: &Names,
    ) -> Result<()> {
        let file = loot_tables.join(format!("{path}.json"));
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        self.add(&format!("minecraft:{path}"), &text, predicates, names)
            .with_context(|| format!("reading the loot table {}", file.display()))
    }

    /// The names of the tables that the tables' `loot_table` entries name.
    fn named_tables(&self) -> Vec<String> {
        fn walk(entries: &[Entry], found: &mut Vec<String>) {
            for entry in entries {
                match entry {
                    Entry::Table { name, .. } => found.push(name.clone()),
                    Entry::Alternatives { children, .. } => walk(children, found),
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        for table in &self.tables {
            for pool in &table.pools {
                walk(&pool.entries, &mut found);
            }
        }
        found.sort();
        found.dedup();
        found
    }

    /// Reads what smelting makes of each item from the recipes of `recipes` (the
    /// directory `recipe`): every `smelting` recipe of one item, or of a list of items.
    /// A recipe of a tag (logs to charcoal, sand to glass) is left out: no loot is of
    /// one, and nothing else here reads it.
    pub fn add_smelting(&mut self, recipes: &Path, names: &Names) -> Result<()> {
        let mut smelting = BTreeMap::new();
        for stem in tables_under(recipes)? {
            let file = recipes.join(format!("{stem}.json"));
            let text =
                fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let json = Json::parse(&text)?;
            if kind(&json)? != "smelting" {
                continue;
            }
            let result = names.item(json.member("result")?.member("id")?.string()?)?;
            let ingredients = match json.member("ingredient")? {
                Json::String(name) if name.starts_with('#') => continue,
                Json::String(name) => vec![name.as_str()],
                Json::Array(listed) => listed
                    .iter()
                    .map(Json::string)
                    .collect::<Result<Vec<_>>>()?,
                other => bail!("the ingredient {other:?} of {stem} is not understood"),
            };
            for ingredient in ingredients {
                let item = names.item(ingredient)?;
                ensure!(
                    smelting
                        .insert(item, result)
                        .is_none_or(|was| was == result),
                    "{ingredient} smelts to two things"
                );
            }
        }
        self.smelting = smelting.into_iter().collect();
        Ok(())
    }
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
        // A table's type says which parameters it is given: a block's, an entity's, the
        // fishing a guardian's table names, a shearing's, or a chicken's gift of an egg.
        let expected = if name.starts_with("minecraft:blocks/") {
            "block"
        } else if name.starts_with("minecraft:entities/") {
            "entity"
        } else if name.starts_with("minecraft:gameplay/fishing") {
            "fishing"
        } else if name.starts_with("minecraft:shearing/") {
            "shearing"
        } else if name == "minecraft:gameplay/chicken_lay" {
            "gift"
        } else {
            bail!("the table {name} is of no kind read here")
        };
        ensure!(
            kind(&json)? == expected,
            "the table {name} is of the type {expected}"
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
        // A constant, or a witch's uniform.
        let rolls = int_provider(pool.member("rolls")?).context("the rolls of a pool")?;
        ensure!(
            !matches!(rolls, IntProvider::Binomial { .. }),
            "the rolls of a pool are a constant or uniform"
        );
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
        // A singleton's weight, 1 by default; no table has a quality.
        let weight = entry.get("weight").map_or(Ok(1), Json::integer)?;
        Ok(match kind(entry)? {
            "item" => {
                only(entry, &["type", "name", "condition", "modifier", "weight"])?;
                Entry::Item {
                    item: names.item(entry.member("name")?.string()?)?,
                    weight,
                    condition,
                    modifiers: self.modifiers(entry.get("modifier"), predicates, names)?,
                }
            }
            "empty" => {
                only(entry, &["type", "condition", "weight"])?;
                Entry::Empty { weight, condition }
            }
            "loot_table" => {
                only(entry, &["type", "value", "condition", "modifier", "weight"])?;
                Entry::Table {
                    name: entry
                        .member("value")?
                        .string()
                        .context("a nested table is named, not written out")?
                        .to_owned(),
                    weight,
                    condition,
                    modifiers: self.modifiers(entry.get("modifier"), predicates, names)?,
                }
            }
            "tag" => {
                only(
                    entry,
                    &["type", "items", "expand", "condition", "modifier", "weight"],
                )?;
                let tag = entry.member("items")?.string()?;
                ensure!(tag.starts_with('#'), "a tag entry names a tag");
                Entry::Tag {
                    tag: tag.to_owned(),
                    expand: entry.member("expand")?.boolean()?,
                    weight,
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
                let entity = value.member("entity")?.string()?;
                ensure!(
                    ["this", "attacker", "direct_attacker", "attacking_player"].contains(&entity),
                    "the entity {entity} of a loot context is not understood"
                );
                let predicate = value.member("predicate")?;
                if predicate.members()?.is_empty() {
                    Condition::EntityProperties {
                        entity: entity.to_owned(),
                    }
                } else {
                    Condition::EntityMatches {
                        entity: entity.to_owned(),
                        predicate: entity_predicate(predicate, names)?,
                    }
                }
            }
            "damage_source_properties" => {
                only(value, &["type", "predicate"])?;
                Condition::DamageSource(damage_source_predicate(value.member("predicate")?, names)?)
            }
            "killed_by_player" => {
                only(value, &["type"])?;
                Condition::KilledByPlayer
            }
            "random_chance_with_enchanted_bonus" => {
                only(
                    value,
                    &[
                        "type",
                        "enchantment",
                        "unenchanted_chance",
                        "enchanted_chance",
                    ],
                )?;
                let enchanted = value.member("enchanted_chance")?;
                let (base, per_level) = match enchanted {
                    Json::Number(_) => (float_bits(enchanted)?, 0.0f32.to_bits()),
                    _ => {
                        ensure!(
                            kind(enchanted)? == "linear",
                            "a level-based chance of the type {} is not understood",
                            kind(enchanted)?
                        );
                        only(enchanted, &["type", "base", "per_level_above_first"])?;
                        (
                            float_bits(enchanted.member("base")?)?,
                            float_bits(enchanted.member("per_level_above_first")?)?,
                        )
                    }
                };
                Condition::RandomChanceWithEnchantedBonus {
                    enchantment: value.member("enchantment")?.string()?.to_owned(),
                    unenchanted: float_bits(value.member("unenchanted_chance")?)?,
                    base,
                    per_level,
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
            "enchanted_count_increase" => {
                only(
                    value,
                    &["type", "count", "enchantment", "limit", "condition"],
                )?;
                let count = value.member("count")?;
                ensure!(
                    !matches!(count, Json::Number(_)) && kind(count)? == "uniform",
                    "a count increase that is not a uniform float is not understood"
                );
                only(count, &["type", "min", "max"])?;
                ModifierKind::EnchantedCountIncrease {
                    enchantment: value.member("enchantment")?.string()?.to_owned(),
                    min: float_bits(count.member("min")?)?,
                    max: float_bits(count.member("max")?)?,
                    limit: value.get("limit").map_or(Ok(0), Json::integer)?,
                }
            }
            "furnace_smelt" => {
                only(value, &["type", "condition"])?;
                ModifierKind::FurnaceSmelt
            }
            "set_potion" => {
                only(value, &["type", "id", "condition"])?;
                ModifierKind::SetPotion {
                    potion: value.member("id")?.string()?.to_owned(),
                }
            }
            "set_ominous_bottle_amplifier" => {
                only(value, &["type", "amplifier", "condition"])?;
                ModifierKind::SetOminousBottleAmplifier {
                    amplifier: int_provider(value.member("amplifier")?)?,
                }
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

/// An `EntityPredicate` of the parts the game's loot tables use.
fn entity_predicate(predicate: &Json, names: &Names) -> Result<EntityPredicate> {
    let mut read = EntityPredicate::default();
    for (part, value) in predicate.members()? {
        match part.as_str() {
            "minecraft:entity_type" => read.kind = Some(names.entity_kind(value.string()?)?),
            "minecraft:flags" => {
                only(value, &["is_on_fire", "is_baby"])?;
                read.on_fire = value.get("is_on_fire").map(Json::boolean).transpose()?;
                read.baby = value.get("is_baby").map(Json::boolean).transpose()?;
            }
            "minecraft:vehicle" => {
                only(value, &["minecraft:entity_type"])?;
                read.vehicle =
                    Some(names.entity_kind(value.member("minecraft:entity_type")?.string()?)?);
            }
            "minecraft:components" => {
                only(
                    value,
                    &[
                        "minecraft:sheep/color",
                        "minecraft:frog/variant",
                        "minecraft:mooshroom/variant",
                        "minecraft:chicken/variant",
                    ],
                )?;
                let text = |key: &str| {
                    value
                        .get(key)
                        .map(|named| named.string().map(str::to_owned))
                        .transpose()
                };
                read.sheep_colour = text("minecraft:sheep/color")?;
                read.frog_variant = text("minecraft:frog/variant")?;
                read.mooshroom_variant = text("minecraft:mooshroom/variant")?;
                read.chicken_variant = text("minecraft:chicken/variant")?;
            }
            "minecraft:type_specific/sheep" => {
                only(value, &["sheared"])?;
                read.sheared = Some(value.member("sheared")?.boolean()?);
            }
            "minecraft:type_specific/raider" => {
                only(value, &["is_captain"])?;
                read.captain = Some(value.member("is_captain")?.boolean()?);
            }
            "minecraft:type_specific/cube_mob" => {
                only(value, &["size"])?;
                let size = value.member("size")?;
                read.cube_size = Some(match size {
                    Json::Number(_) => (Some(size.integer()?), Some(size.integer()?)),
                    _ => {
                        only(size, &["min", "max"])?;
                        (
                            size.get("min").map(Json::integer).transpose()?,
                            size.get("max").map(Json::integer).transpose()?,
                        )
                    }
                });
            }
            "minecraft:equipment" => {
                only(value, &["mainhand"])?;
                let hand = value.member("mainhand")?;
                only(hand, &["predicates"])?;
                let predicates = hand.member("predicates")?;
                only(predicates, &["minecraft:enchantments"])?;
                let [enchantment] = predicates.member("minecraft:enchantments")?.array()? else {
                    bail!("a held item's predicate of other than one enchantment");
                };
                only(enchantment, &["enchantments"])?;
                read.main_hand_enchantment =
                    Some(enchantment.member("enchantments")?.string()?.to_owned());
            }
            other => bail!("the entity predicate's {other} is not understood"),
        }
    }
    Ok(read)
}

/// A `DamageSourcePredicate` of the parts the game's loot tables use.
fn damage_source_predicate(predicate: &Json, names: &Names) -> Result<DamageSourcePredicate> {
    only(predicate, &["tags", "source_entity", "direct_entity"])?;
    let mut tags = Vec::new();
    if let Some(listed) = predicate.get("tags") {
        for tag in listed.array()? {
            only(tag, &["id", "expected"])?;
            let id = tag.member("id")?.string()?;
            ensure!(id.starts_with('#'), "a damage tag predicate names a tag");
            tags.push((id.to_owned(), tag.member("expected")?.boolean()?));
        }
    }
    let entity = |key: &str| -> Result<Option<EntityPredicate>> {
        predicate
            .get(key)
            .map(|value| entity_predicate(value, names))
            .transpose()
    };
    Ok(DamageSourcePredicate {
        tags,
        source_entity: entity("source_entity")?,
        direct_entity: entity("direct_entity")?,
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

/// `loot_tables.rs`: the conditions, the modifiers and the tables; for each block and
/// each entity type the number of its table; and what smelting makes. `block_tables`
/// are the names of the blocks' tables by block id, `entity_tables` those of the
/// entity types' by type id.
pub fn emit(
    compiled: &Compiled,
    block_tables: &[Option<&str>],
    entity_tables: &[Option<&str>],
) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "use crate::loot::{{BonusFormula, DamageSourcePredicate, EntityPredicate, IntProvider, \
         LootCondition, LootEntry, LootModifier, LootPool, LootTable, StatePredicate}};\n"
    )?;
    writeln!(
        out,
        "/// The distinct conditions of the loot tables, which they name by number."
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
        "/// The distinct item modifiers of the loot tables, which they name by number."
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
    writeln!(out, "/// The loot tables, by name.")?;
    writeln!(
        out,
        "pub static TABLES: [LootTable; {}] = [",
        compiled.tables.len()
    )?;
    ensure!(
        compiled.tables.len() < usize::from(u16::MAX),
        "more loot tables than 16 bits can number"
    );
    ensure!(
        compiled.tables.is_sorted_by(|a, b| a.name < b.name),
        "the tables are not sorted by name"
    );
    for table in &compiled.tables {
        let mut pools = Vec::new();
        for pool in &table.pools {
            let entries = pool
                .entries
                .iter()
                .map(|entry| entry_text(entry, compiled))
                .collect::<Result<Vec<_>>>()?;
            pools.push(format!(
                "LootPool {{ rolls: {}, condition: {}, entries: &[{}], modifiers: &[{}] }}",
                int_text(pool.rolls),
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
    out.push_str(&crate::emit::wrapped(
        &table_numbers(compiled, block_tables),
        "    ",
    ));
    writeln!(out, "];\n")?;

    writeln!(
        out,
        "/// The number of each entity type's table in `TABLES`, by entity type id; \
         `u16::MAX` for a\n/// type that has none, which drops nothing when it dies."
    )?;
    writeln!(
        out,
        "pub static ENTITY_TABLES: [u16; {}] = [",
        entity_tables.len()
    )?;
    out.push_str(&crate::emit::wrapped(
        &table_numbers(compiled, entity_tables),
        "    ",
    ));
    writeln!(out, "];\n")?;

    writeln!(
        out,
        "/// What smelting makes of an item (`furnace_smelt`), by item id, ascending."
    )?;
    writeln!(
        out,
        "pub static SMELTING: [(u16, u16); {}] = [",
        compiled.smelting.len()
    )?;
    for &(item, result) in &compiled.smelting {
        writeln!(out, "    ({}, {}),", small(item)?, small(result)?)?;
    }
    writeln!(out, "];")?;
    Ok(out)
}

/// The numbers in the tables of the tables `names` names, `u16::MAX` where it names
/// none or one that is not there.
fn table_numbers(compiled: &Compiled, names: &[Option<&str>]) -> Vec<usize> {
    names
        .iter()
        .map(|name| {
            name.and_then(|name| {
                compiled
                    .tables
                    .binary_search_by(|table| table.name.as_str().cmp(name))
                    .ok()
            })
            .unwrap_or(usize::from(u16::MAX))
        })
        .collect()
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

fn optional_text<T>(value: &Option<T>, text: impl Fn(&T) -> String) -> String {
    match value {
        Some(value) => format!("Some({})", text(value)),
        None => "None".to_owned(),
    }
}

fn int_text(provider: IntProvider) -> String {
    match provider {
        IntProvider::Constant(value) => format!("IntProvider::Constant({value})"),
        IntProvider::Uniform { min, max } => {
            format!("IntProvider::Uniform {{ min: {min}, max: {max} }}")
        }
        IntProvider::Binomial { n, p } => {
            format!("IntProvider::Binomial {{ n: {n}, p: {} }}", float(p))
        }
    }
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

fn entity_predicate_text(predicate: &EntityPredicate) -> String {
    let text = |value: &String| format!("{value:?}");
    let flag = |value: &bool| value.to_string();
    let size = |(min, max): &(Option<i32>, Option<i32>)| format!("({min:?}, {max:?})");
    format!(
        "EntityPredicate {{ kind: {}, on_fire: {}, baby: {}, vehicle: {}, sheep_colour: {}, \
         frog_variant: {}, mooshroom_variant: {}, chicken_variant: {}, sheared: {}, \
         cube_size: {}, captain: {}, main_hand_enchantment: {} }}",
        optional_text(&predicate.kind, text),
        optional_text(&predicate.on_fire, flag),
        optional_text(&predicate.baby, flag),
        optional_text(&predicate.vehicle, text),
        optional_text(&predicate.sheep_colour, text),
        optional_text(&predicate.frog_variant, text),
        optional_text(&predicate.mooshroom_variant, text),
        optional_text(&predicate.chicken_variant, text),
        optional_text(&predicate.sheared, flag),
        optional_text(&predicate.cube_size, size),
        optional_text(&predicate.captain, flag),
        optional_text(&predicate.main_hand_enchantment, text),
    )
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
        Condition::KilledByPlayer => "LootCondition::KilledByPlayer".to_owned(),
        Condition::RandomChanceWithEnchantedBonus {
            enchantment,
            unenchanted,
            base,
            per_level,
        } => format!(
            "LootCondition::RandomChanceWithEnchantedBonus {{ enchantment: {enchantment:?}, \
             unenchanted: {}, base: {}, per_level: {} }}",
            float(*unenchanted),
            float(*base),
            float(*per_level)
        ),
        Condition::EntityMatches { entity, predicate } => format!(
            "LootCondition::EntityMatches {{ entity: {entity:?}, predicate: {} }}",
            entity_predicate_text(predicate)
        ),
        Condition::DamageSource(predicate) => {
            let tags: Vec<String> = predicate
                .tags
                .iter()
                .map(|(tag, expected)| format!("({tag:?}, {expected})"))
                .collect();
            format!(
                "LootCondition::DamageSource(DamageSourcePredicate {{ tags: &[{}], \
                 source_entity: {}, direct_entity: {} }})",
                tags.join(", "),
                optional_text(&predicate.source_entity, entity_predicate_text),
                optional_text(&predicate.direct_entity, entity_predicate_text)
            )
        }
    })
}

fn modifier_text(modifier: &Modifier) -> Result<String> {
    let condition = option(modifier.condition)?;
    Ok(match &modifier.kind {
        ModifierKind::SetCount { count, add } => format!(
            "LootModifier::SetCount {{ count: {}, add: {add}, condition: {condition} }}",
            int_text(*count)
        ),
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
        ModifierKind::EnchantedCountIncrease {
            enchantment,
            min,
            max,
            limit,
        } => format!(
            "LootModifier::EnchantedCountIncrease {{ enchantment: {enchantment:?}, min: {}, \
             max: {}, limit: {limit}, condition: {condition} }}",
            float(*min),
            float(*max)
        ),
        ModifierKind::FurnaceSmelt => {
            format!("LootModifier::FurnaceSmelt {{ condition: {condition} }}")
        }
        ModifierKind::SetPotion { potion } => {
            format!("LootModifier::SetPotion {{ potion: {potion:?}, condition: {condition} }}")
        }
        ModifierKind::SetOminousBottleAmplifier { amplifier } => format!(
            "LootModifier::SetOminousBottleAmplifier {{ amplifier: {}, condition: {condition} }}",
            int_text(*amplifier)
        ),
    })
}

fn entry_text(entry: &Entry, compiled: &Compiled) -> Result<String> {
    Ok(match entry {
        Entry::Item {
            item,
            weight,
            condition,
            modifiers,
        } => format!(
            "LootEntry::Item {{ item: {}, weight: {weight}, condition: {}, modifiers: &[{}] }}",
            small(*item)?,
            option(*condition)?,
            numbers(modifiers)?
        ),
        Entry::Empty { weight, condition } => format!(
            "LootEntry::Empty {{ weight: {weight}, condition: {} }}",
            option(*condition)?
        ),
        Entry::Table {
            name,
            weight,
            condition,
            modifiers,
        } => {
            let table = compiled
                .tables
                .binary_search_by(|table| table.name.as_str().cmp(name))
                .ok()
                .with_context(|| format!("there is no table {name}"))?;
            format!(
                "LootEntry::Table {{ table: {}, weight: {weight}, condition: {}, \
                 modifiers: &[{}] }}",
                small(table)?,
                option(*condition)?,
                numbers(modifiers)?
            )
        }
        Entry::Tag {
            tag,
            expand,
            weight,
            condition,
            modifiers,
        } => format!(
            "LootEntry::Tag {{ tag: {tag:?}, expand: {expand}, weight: {weight}, condition: {}, \
             modifiers: &[{}] }}",
            option(*condition)?,
            numbers(modifiers)?
        ),
        Entry::Alternatives {
            condition,
            children,
        } => {
            let children = children
                .iter()
                .map(|child| entry_text(child, compiled))
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
            "minecraft:beef",
            "minecraft:cooked_beef",
            "minecraft:leather",
            "minecraft:rabbit_foot",
            "minecraft:white_wool",
            "minecraft:egg",
            "minecraft:blue_egg",
            "minecraft:red_mushroom",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn entities() -> Vec<String> {
        ["minecraft:cow", "minecraft:sheep", "minecraft:frog"]
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
        let entities = entities();
        let names = Names {
            blocks: &blocks,
            items: &items,
            entities: &entities,
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

        let text = emit(&compiled, &[Some("minecraft:blocks/oak_leaves"), None], &[]).unwrap();
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
            // A rolls that is neither a constant nor uniform.
            LEAVES.replacen(
                "\"rolls\": 1",
                "\"rolls\": {\"type\": \"minecraft:binomial\", \"n\": 1, \"p\": 0.5}",
                1,
            ),
            // A block's table of the type of an entity's.
            LEAVES.replace("minecraft:block\"", "minecraft:entity\""),
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

    const COW: &str = r##"{
      "type": "minecraft:entity",
      "pools": [
        {"entries": [{"type": "minecraft:item", "modifier": [
          {"type": "minecraft:set_count", "count": {"type": "minecraft:uniform", "max": 2, "min": 0}},
          {"type": "minecraft:enchanted_count_increase", "count":
            {"type": "minecraft:uniform", "max": 1.0, "min": 0.0}, "enchantment": "minecraft:looting"}],
          "name": "minecraft:leather"}], "rolls": 1},
        {"entries": [{"type": "minecraft:item", "modifier": [
          {"type": "minecraft:furnace_smelt", "condition": {"type": "minecraft:any_of", "terms": [
            {"type": "minecraft:entity_properties", "entity": "this", "predicate":
              {"minecraft:flags": {"is_on_fire": true}}},
            {"type": "minecraft:entity_properties", "entity": "direct_attacker", "predicate":
              {"minecraft:equipment": {"mainhand": {"predicates": {"minecraft:enchantments":
                [{"enchantments": "#minecraft:smelts_loot"}]}}}}}]}}],
          "name": "minecraft:beef"}], "rolls": 1},
        {"condition": {"type": "minecraft:all_of", "terms": [
          {"type": "minecraft:killed_by_player"},
          {"type": "minecraft:random_chance_with_enchanted_bonus", "enchanted_chance":
            {"type": "minecraft:linear", "base": 0.13, "per_level_above_first": 0.03},
           "enchantment": "minecraft:looting", "unenchanted_chance": 0.1}]},
         "entries": [{"type": "minecraft:item", "name": "minecraft:rabbit_foot", "weight": 3},
           {"type": "minecraft:empty", "weight": 2},
           {"type": "minecraft:loot_table", "value": "minecraft:entities/sheep/white",
            "condition": {"type": "minecraft:entity_properties", "entity": "this", "predicate":
              {"minecraft:components": {"minecraft:sheep/color": "white"},
               "minecraft:type_specific/sheep": {"sheared": false}}}},
           {"type": "minecraft:item", "name": "minecraft:stick", "condition":
             {"type": "minecraft:damage_source_properties", "predicate":
               {"source_entity": {"minecraft:entity_type": "minecraft:frog",
                 "minecraft:components": {"minecraft:frog/variant": "minecraft:warm"}},
                "tags": [{"expected": true, "id": "#minecraft:is_lightning"}]}}}],
         "rolls": {"type": "minecraft:uniform", "max": 3, "min": 1}}
      ],
      "random_sequence": "minecraft:entities/cow"
    }"##;

    const WHITE_WOOL: &str = r#"{"type": "minecraft:entity", "pools": [{"entries":
      [{"type": "minecraft:item", "name": "minecraft:white_wool"}], "rolls": 1}],
      "random_sequence": "minecraft:entities/sheep/white"}"#;

    fn compile_entities(tables: &[(&str, &str)]) -> Result<Compiled> {
        let blocks = blocks();
        let items = items();
        let entities = entities();
        let names = Names {
            blocks: &blocks,
            items: &items,
            entities: &entities,
        };
        let mut compiled = Compiled::default();
        for (name, text) in tables {
            compiled.add(name, text, &Files, &names)?;
        }
        compiled.tables.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(compiled)
    }

    /// An entity's table: counts raised by looting, a smelt by the mob's fire or the
    /// weapon's enchantment, a player's kill and looting's chance, weights, nothing,
    /// a nested table under a sheep's colour, and a damage source's cause and tags.
    #[test]
    fn an_entity_s_table_is_compiled_with_what_an_entity_s_loot_reads() {
        let compiled = compile_entities(&[
            ("minecraft:entities/cow", COW),
            ("minecraft:entities/sheep/white", WHITE_WOOL),
        ])
        .unwrap();
        let cow = &compiled.tables[0];
        assert_eq!(cow.name, "minecraft:entities/cow");
        let Entry::Item { modifiers, .. } = &cow.pools[0].entries[0] else {
            panic!("leather");
        };
        assert_eq!(
            compiled.modifiers[modifiers[1]].kind,
            ModifierKind::EnchantedCountIncrease {
                enchantment: "minecraft:looting".to_owned(),
                min: 0.0f32.to_bits(),
                max: 1.0f32.to_bits(),
                limit: 0
            }
        );
        let Entry::Item { modifiers, .. } = &cow.pools[1].entries[0] else {
            panic!("beef");
        };
        let smelt = &compiled.modifiers[modifiers[0]];
        assert_eq!(smelt.kind, ModifierKind::FurnaceSmelt);
        let Condition::AnyOf(terms) = &compiled.conditions[smelt.condition.unwrap()] else {
            panic!("burning or the weapon's");
        };
        assert_eq!(
            compiled.conditions[terms[0]],
            Condition::EntityMatches {
                entity: "this".to_owned(),
                predicate: EntityPredicate {
                    on_fire: Some(true),
                    ..EntityPredicate::default()
                }
            }
        );
        assert_eq!(
            compiled.conditions[terms[1]],
            Condition::EntityMatches {
                entity: "direct_attacker".to_owned(),
                predicate: EntityPredicate {
                    main_hand_enchantment: Some("#minecraft:smelts_loot".to_owned()),
                    ..EntityPredicate::default()
                }
            }
        );
        let pool = &cow.pools[2];
        assert_eq!(pool.rolls, IntProvider::Uniform { min: 1, max: 3 });
        let Condition::AllOf(terms) = &compiled.conditions[pool.condition.unwrap()] else {
            panic!("a player's kill and a chance");
        };
        assert_eq!(compiled.conditions[terms[0]], Condition::KilledByPlayer);
        assert_eq!(
            compiled.conditions[terms[1]],
            Condition::RandomChanceWithEnchantedBonus {
                enchantment: "minecraft:looting".to_owned(),
                unenchanted: 0.1f32.to_bits(),
                base: 0.13f32.to_bits(),
                per_level: 0.03f32.to_bits()
            }
        );
        assert!(matches!(pool.entries[0], Entry::Item { weight: 3, .. }));
        assert!(matches!(pool.entries[1], Entry::Empty { weight: 2, .. }));
        let Entry::Table {
            name, condition, ..
        } = &pool.entries[2]
        else {
            panic!("the wool's table");
        };
        assert_eq!(name, "minecraft:entities/sheep/white");
        assert_eq!(
            compiled.conditions[condition.unwrap()],
            Condition::EntityMatches {
                entity: "this".to_owned(),
                predicate: EntityPredicate {
                    sheep_colour: Some("white".to_owned()),
                    sheared: Some(false),
                    ..EntityPredicate::default()
                }
            }
        );
        let Entry::Item { condition, .. } = &pool.entries[3] else {
            panic!("a stick");
        };
        assert_eq!(
            compiled.conditions[condition.unwrap()],
            Condition::DamageSource(DamageSourcePredicate {
                tags: vec![("#minecraft:is_lightning".to_owned(), true)],
                source_entity: Some(EntityPredicate {
                    kind: Some("minecraft:frog".to_owned()),
                    frog_variant: Some("minecraft:warm".to_owned()),
                    ..EntityPredicate::default()
                }),
                direct_entity: None,
            })
        );

        let text = emit(
            &compiled,
            &[],
            &[
                Some("minecraft:entities/cow"),
                Some("minecraft:entities/sheep"),
                Some("minecraft:entities/frog"),
            ],
        )
        .unwrap();
        // The nested table is named by its number among the tables.
        assert!(
            text.contains("LootEntry::Table { table: 1, weight: 1,"),
            "{text}"
        );
        assert!(text.contains("pub static ENTITY_TABLES: [u16; 3] = [\n    0, 65535, 65535,\n];"));
        assert!(text.contains("rolls: IntProvider::Uniform { min: 1, max: 3 }"));
    }

    const CHICKEN_LAY: &str = r#"{"type": "minecraft:gift", "pools": [{"entries": [
      {"type": "minecraft:alternatives", "children": [
        {"type": "minecraft:item", "name": "minecraft:egg", "condition":
          {"type": "minecraft:entity_properties", "entity": "this", "predicate":
            {"minecraft:components": {"minecraft:chicken/variant": "minecraft:temperate"}}}},
        {"type": "minecraft:item", "name": "minecraft:blue_egg", "condition":
          {"type": "minecraft:entity_properties", "entity": "this", "predicate":
            {"minecraft:components": {"minecraft:chicken/variant": "minecraft:cold"}}}}]}],
      "rolls": 1}], "random_sequence": "minecraft:gameplay/chicken_lay"}"#;

    const MOOSHROOM: &str = r#"{"type": "minecraft:shearing", "pools": [{"entries": [
      {"type": "minecraft:alternatives", "children": [
        {"type": "minecraft:loot_table", "value": "minecraft:shearing/mooshroom/red",
         "condition": {"type": "minecraft:entity_properties", "entity": "this",
           "predicate": {"minecraft:components": {"minecraft:mooshroom/variant": "red"}}}}]}],
      "rolls": 1}], "random_sequence": "minecraft:shearing/mooshroom"}"#;

    const RED_MUSHROOMS: &str = r#"{"type": "minecraft:shearing", "pools": [{"entries":
      [{"type": "minecraft:item", "name": "minecraft:red_mushroom"}], "rolls": 5}],
      "random_sequence": "minecraft:shearing/mooshroom/red"}"#;

    /// The tables of the farm animals' own code: a gift and a shearing, which pick by a
    /// chicken's and a mooshroom's variant, and a shearing's table of its colour.
    #[test]
    fn a_chicken_s_laying_and_a_mooshroom_s_shearing_are_compiled_by_their_variants() {
        let compiled = compile_entities(&[
            ("minecraft:gameplay/chicken_lay", CHICKEN_LAY),
            ("minecraft:shearing/mooshroom", MOOSHROOM),
            ("minecraft:shearing/mooshroom/red", RED_MUSHROOMS),
        ])
        .unwrap();
        let names: Vec<&str> = compiled
            .tables
            .iter()
            .map(|table| table.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "minecraft:gameplay/chicken_lay",
                "minecraft:shearing/mooshroom",
                "minecraft:shearing/mooshroom/red"
            ]
        );
        let Entry::Alternatives { children, .. } = &compiled.tables[0].pools[0].entries[0] else {
            panic!("the eggs are alternatives");
        };
        let Entry::Item {
            condition: Some(cold),
            ..
        } = &children[1]
        else {
            panic!("a blue egg under a condition");
        };
        assert_eq!(
            compiled.conditions[*cold],
            Condition::EntityMatches {
                entity: "this".to_owned(),
                predicate: EntityPredicate {
                    chicken_variant: Some("minecraft:cold".to_owned()),
                    ..EntityPredicate::default()
                },
            }
        );
        let Entry::Alternatives { children, .. } = &compiled.tables[1].pools[0].entries[0] else {
            panic!("the colours are alternatives");
        };
        let Entry::Table {
            name,
            condition: Some(red),
            ..
        } = &children[0]
        else {
            panic!("the red table under a condition");
        };
        assert_eq!(name, "minecraft:shearing/mooshroom/red");
        assert_eq!(
            compiled.conditions[*red],
            Condition::EntityMatches {
                entity: "this".to_owned(),
                predicate: EntityPredicate {
                    mooshroom_variant: Some("red".to_owned()),
                    ..EntityPredicate::default()
                },
            }
        );
        assert_eq!(compiled.tables[2].pools[0].rolls, IntProvider::Constant(5));
        let text = emit(&compiled, &[], &[]).unwrap();
        assert!(text.contains("mooshroom_variant: Some(\"red\")"), "{text}");
        assert!(text.contains("chicken_variant: Some(\"minecraft:cold\")"));
        // A table of either kind under a name of the other is refused, as is a variant
        // of a mob nobody reads.
        for (name, text) in [
            (
                "minecraft:gameplay/chicken_lay",
                MOOSHROOM.replace("shearing/mooshroom\"", "gameplay/chicken_lay\""),
            ),
            (
                "minecraft:shearing/mooshroom",
                MOOSHROOM.replace("mooshroom/variant", "goat/variant"),
            ),
        ] {
            assert!(compile_entities(&[(name, &text)]).is_err(), "{name}");
        }
    }

    #[test]
    fn what_an_entity_s_table_does_not_understand_is_refused() {
        let refused = [
            // A predicate part nobody reads, a flag nobody reads.
            COW.replace("minecraft:flags", "minecraft:location"),
            COW.replace("is_on_fire", "is_sprinting"),
            // An entity of the context that is not one, a type that is not one.
            COW.replace("direct_attacker", "killer_entity"),
            COW.replace("minecraft:frog\"", "minecraft:toad\""),
            // A quality, a modifier that is not understood.
            COW.replace("\"weight\": 3", "\"weight\": 3, \"quality\": 1"),
            COW.replace("minecraft:furnace_smelt", "minecraft:enchant_with_levels"),
            // A level-based chance of another kind.
            COW.replace("minecraft:linear", "minecraft:clamped"),
        ];
        for (index, text) in refused.iter().enumerate() {
            assert_ne!(text, COW, "case {index} changed nothing");
            let compiled = compile_entities(&[
                ("minecraft:entities/cow", text),
                ("minecraft:entities/sheep/white", WHITE_WOOL),
            ]);
            assert!(compiled.is_err(), "case {index} was accepted");
        }
        // A nested table that is not there.
        let compiled = compile_entities(&[("minecraft:entities/cow", COW)]).unwrap();
        assert!(emit(&compiled, &[], &[]).is_err());
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
        let entities = model::static_registry(&registries, "entity_type").unwrap();
        let names = Names {
            blocks: &blocks,
            items: &items,
            entities: &entities,
        };
        let predicates = PredicateDirectory(&data.join("predicate"));
        let mut compiled = load(&data.join("loot_table/blocks"), &predicates, &names).unwrap();
        assert_eq!(compiled.tables.len(), 1201);
        // The entities' 93 and the sheep's 16, and the guardians' fish; the chicken's
        // laying, the mooshroom's shearing and its two colours', the sheep's shearing and
        // its sixteen colours'.
        compiled
            .add_entities(&data.join("loot_table"), &predicates, &names)
            .unwrap();
        compiled.add_smelting(&data.join("recipe"), &names).unwrap();
        assert_eq!(compiled.tables.len(), 1201 + 93 + 16 + 1 + 1 + 3 + 17);
        let by_name: Vec<String> = block_names
            .iter()
            .map(|name| name.replace("minecraft:", "minecraft:blocks/"))
            .collect();
        let tables: Vec<Option<&str>> = by_name.iter().map(|name| Some(name.as_str())).collect();
        let entity_names: Vec<String> = entities
            .iter()
            .map(|name| name.replacen("minecraft:", "minecraft:entities/", 1))
            .collect();
        let entity_tables: Vec<Option<&str>> = entity_names
            .iter()
            .map(|name| Some(name.as_str()))
            .collect();
        let text = emit(&compiled, &tables, &entity_tables).unwrap();
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
