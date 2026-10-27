//! Compiles what enchanting reads of the enchantments into `enchantments.rs`, which
//! `clustine-data`'s `enchanting` reads (`docs/groundwork/n10-plan.md`, decision 8 and
//! section 8.1): each enchantment's weight, levels, costs, exclusive set, supported and
//! primary items (`data/minecraft/enchantment/*.json`, `Enchantment$EnchantmentDefinition`
//! and `Enchantment$Cost`), and each item's enchantability (`minecraft:enchantable` in
//! the generator's report of the item's components).
//!
//! Only these fields are read; an enchantment's effects, slots and anvil cost are the
//! rules' and are not compiled. A set the reader cannot place (a name that is no item or
//! enchantment, a tag that does not exist) or a cost of another shape fails the run, so
//! that a jar that changes one is noticed.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;
use crate::loot::tables_under;
use crate::trades::{ItemSet, NamedSet};

/// `Enchantment$Cost`: `base + per_level_above_first · (level − 1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    pub base: i32,
    pub per_level_above_first: i32,
}

/// What enchanting reads of one enchantment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enchantment {
    pub name: String,
    pub weight: i32,
    pub max_level: i32,
    pub min_cost: Cost,
    pub max_cost: Cost,
    pub exclusive_set: Option<NamedSet>,
    pub supported_items: ItemSet,
    pub primary_items: Option<ItemSet>,
}

/// The enchantments by name, and the items that can be enchanted by id with their
/// enchantability.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compiled {
    pub enchantments: Vec<Enchantment>,
    pub enchantable: Vec<(usize, i32)>,
}

/// The names a set may hold: items by id in order, enchantments by name, and the tags
/// of each with `#`.
pub struct Names<'a> {
    pub items: &'a [String],
    pub enchantments: &'a [String],
    pub item_tags: &'a [String],
    pub enchantment_tags: &'a [String],
}

/// Reads the enchantments of `data` (`data/minecraft`) and the enchantability of each
/// item from `item_reports` (`reports/minecraft/components/item`).
pub fn load(data: &Path, item_reports: &Path, names: &Names) -> Result<Compiled> {
    let directory = data.join("enchantment");
    let mut files = Vec::new();
    for path in tables_under(&directory)? {
        let file = directory.join(format!("{path}.json"));
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        files.push((format!("minecraft:{path}"), text));
    }
    let mut reports = Vec::new();
    for name in names.items {
        let local = name.strip_prefix("minecraft:").unwrap_or(name);
        let file = item_reports.join(format!("{local}.json"));
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        reports.push(text);
    }
    compile(&files, &reports, names)
}

/// Compiles the enchantments, each a file's name and text, and the items' component
/// reports, one an item in the order of `names.items`.
pub fn compile(
    enchantments: &[(String, String)],
    reports: &[String],
    names: &Names,
) -> Result<Compiled> {
    ensure!(
        reports.len() == names.items.len(),
        "a component report for each item"
    );
    let mut compiled = Compiled::default();
    for (name, text) in enchantments {
        let json = Json::parse(text).with_context(|| format!("reading the enchantment {name}"))?;
        compiled.enchantments.push(
            enchantment(name, &json, names)
                .with_context(|| format!("reading the enchantment {name}"))?,
        );
    }
    compiled.enchantments.sort_by(|a, b| a.name.cmp(&b.name));
    let read: Vec<&str> = compiled
        .enchantments
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    ensure!(
        read == names.enchantments,
        "the enchantment files are not the registry's entries: {read:?}"
    );
    for (id, text) in reports.iter().enumerate() {
        let json = Json::parse(text)
            .with_context(|| format!("reading the components of {}", names.items[id]))?;
        let components = json.member("components")?;
        if let Some(enchantable) = components.get("minecraft:enchantable") {
            let value = enchantable
                .member("value")?
                .integer()
                .with_context(|| format!("the enchantability of {}", names.items[id]))?;
            // `ExtraCodecs.POSITIVE_INT`.
            ensure!(
                value > 0,
                "{} has an enchantability of {value}",
                names.items[id]
            );
            compiled.enchantable.push((id, value));
        }
    }
    Ok(compiled)
}

fn enchantment(name: &str, json: &Json, names: &Names) -> Result<Enchantment> {
    let cost = |key: &str| -> Result<Cost> {
        let value = json.member(key)?;
        crate::loot::only(value, &["base", "per_level_above_first"])?;
        Ok(Cost {
            base: value.member("base")?.integer()?,
            per_level_above_first: value.member("per_level_above_first")?.integer()?,
        })
    };
    let weight = json.member("weight")?.integer()?;
    let max_level = json.member("max_level")?.integer()?;
    // The codec's ranges (`Enchantment$EnchantmentDefinition`: weight 1 to 1024, a
    // maximum level 1 to 255).
    ensure!((1..=1024).contains(&weight), "a weight of {weight}");
    ensure!(
        (1..=255).contains(&max_level),
        "a maximum level of {max_level}"
    );
    Ok(Enchantment {
        name: name.to_owned(),
        weight,
        max_level,
        min_cost: cost("min_cost")?,
        max_cost: cost("max_cost")?,
        exclusive_set: json
            .get("exclusive_set")
            .map(|set| enchantment_set(set, names))
            .transpose()?,
        supported_items: item_set(json.member("supported_items")?, names)?,
        primary_items: json
            .get("primary_items")
            .map(|set| item_set(set, names))
            .transpose()?,
    })
}

/// A `HolderSet` of enchantments: a tag, one name, or names listed.
fn enchantment_set(json: &Json, names: &Names) -> Result<NamedSet> {
    let known = |name: &str| -> Result<String> {
        ensure!(
            names.enchantments.iter().any(|known| known == name),
            "{name} is not an enchantment"
        );
        Ok(name.to_owned())
    };
    Ok(match json {
        Json::String(tag) if tag.starts_with('#') => {
            ensure!(
                names.enchantment_tags.contains(tag),
                "{tag} is not an enchantment tag"
            );
            NamedSet::Tag(tag.clone())
        }
        Json::String(name) => NamedSet::Listed(vec![known(name)?]),
        Json::Array(listed) => NamedSet::Listed(
            listed
                .iter()
                .map(|name| known(name.string()?))
                .collect::<Result<_>>()?,
        ),
        _ => bail!("a set of enchantments of an unknown form"),
    })
}

/// A `HolderSet` of items: a tag, one id, or ids listed.
fn item_set(json: &Json, names: &Names) -> Result<ItemSet> {
    let id = |name: &str| -> Result<usize> {
        names
            .items
            .iter()
            .position(|item| item == name)
            .with_context(|| format!("{name} is not an item"))
    };
    Ok(match json {
        Json::String(tag) if tag.starts_with('#') => {
            ensure!(names.item_tags.contains(tag), "{tag} is not an item tag");
            ItemSet::Tag(tag.clone())
        }
        Json::String(name) => ItemSet::Listed(vec![id(name)?]),
        Json::Array(listed) => ItemSet::Listed(
            listed
                .iter()
                .map(|name| id(name.string()?))
                .collect::<Result<_>>()?,
        ),
        _ => bail!("a set of items of an unknown form"),
    })
}

/// `enchantments.rs`: the enchantments in the registry's order, then the enchantable
/// items by id, importing only the types it names.
pub fn emit(compiled: &Compiled) -> Result<String> {
    ensure!(
        compiled.enchantable.is_sorted_by(|a, b| a.0 < b.0),
        "the enchantable items are not sorted by id"
    );
    let mut body = String::new();
    writeln!(
        body,
        "/// What enchanting reads of each enchantment, in the registry's order (by name)."
    )?;
    writeln!(
        body,
        "pub static ENCHANTMENTS: [Enchantment; {}] = [",
        compiled.enchantments.len()
    )?;
    for enchantment in &compiled.enchantments {
        writeln!(
            body,
            "    Enchantment {{ name: {:?}, weight: {}, max_level: {}, min_cost: {}, \
             max_cost: {}, exclusive_set: {}, supported_items: {}, primary_items: {} }},",
            enchantment.name,
            enchantment.weight,
            enchantment.max_level,
            cost_text(enchantment.min_cost),
            cost_text(enchantment.max_cost),
            optional(enchantment.exclusive_set.as_ref().map(named_text)),
            items_text(&enchantment.supported_items),
            optional(enchantment.primary_items.as_ref().map(items_text)),
        )?;
    }
    writeln!(body, "];\n")?;
    writeln!(
        body,
        "/// Each item that has `minecraft:enchantable` by default, by id, with its value."
    )?;
    writeln!(
        body,
        "pub static ENCHANTABLE: [(u16, i32); {}] = [",
        compiled.enchantable.len()
    )?;
    for (item, value) in &compiled.enchantable {
        ensure!(*item < usize::from(u16::MAX), "an item id beyond 16 bits");
        writeln!(body, "    ({item}, {value}),")?;
    }
    writeln!(body, "];")?;

    let mut uses = vec!["use crate::enchanting::{Enchantment, EnchantmentCost};".to_owned()];
    let trading: Vec<&str> = ["ItemSet", "NamedSet"]
        .into_iter()
        .filter(|name| body.contains(&format!("{name}::")))
        .collect();
    if !trading.is_empty() {
        uses.push(format!("use crate::trading::{{{}}};", trading.join(", ")));
    }
    Ok(format!("{}\n\n{body}", uses.join("\n")))
}

fn cost_text(cost: Cost) -> String {
    format!(
        "EnchantmentCost {{ base: {}, per_level_above_first: {} }}",
        cost.base, cost.per_level_above_first
    )
}

fn optional(text: Option<String>) -> String {
    text.map_or_else(|| "None".to_owned(), |text| format!("Some({text})"))
}

fn named_text(set: &NamedSet) -> String {
    match set {
        NamedSet::Tag(tag) => format!("NamedSet::Tag({tag:?})"),
        NamedSet::Listed(names) => format!("NamedSet::Listed(&{names:?})"),
    }
}

fn items_text(set: &ItemSet) -> String {
    match set {
        ItemSet::Tag(tag) => format!("ItemSet::Tag({tag:?})"),
        ItemSet::Listed(ids) => format!("ItemSet::Listed(&{ids:?})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    struct Lists {
        items: Vec<String>,
        enchantments: Vec<String>,
        item_tags: Vec<String>,
        enchantment_tags: Vec<String>,
    }

    impl Lists {
        fn small() -> Lists {
            Lists {
                items: strings(&["minecraft:air", "minecraft:book", "minecraft:iron_sword"]),
                enchantments: strings(&["minecraft:mending", "minecraft:sharpness"]),
                item_tags: strings(&["#minecraft:enchantable/durability", "#minecraft:swords"]),
                enchantment_tags: strings(&["#minecraft:exclusive_set/damage"]),
            }
        }

        fn names(&self) -> Names<'_> {
            Names {
                items: &self.items,
                enchantments: &self.enchantments,
                item_tags: &self.item_tags,
                enchantment_tags: &self.enchantment_tags,
            }
        }
    }

    const MENDING: &str = r##"{"anvil_cost": 4, "description": {"translate": "x"},
        "effects": {}, "max_cost": {"base": 75, "per_level_above_first": 25},
        "max_level": 1, "min_cost": {"base": 25, "per_level_above_first": 25},
        "slots": ["any"], "supported_items": "#minecraft:enchantable/durability",
        "weight": 2}"##;
    const SHARPNESS: &str = r##"{"anvil_cost": 1, "description": {"translate": "y"},
        "exclusive_set": "#minecraft:exclusive_set/damage",
        "max_cost": {"base": 21, "per_level_above_first": 11}, "max_level": 5,
        "min_cost": {"base": 1, "per_level_above_first": 11},
        "primary_items": ["minecraft:iron_sword"], "slots": ["mainhand"],
        "supported_items": "#minecraft:swords", "weight": 10}"##;

    fn reports() -> Vec<String> {
        vec![
            r#"{"components": {}}"#.to_owned(),
            r#"{"components": {"minecraft:enchantable": {"value": 1}}}"#.to_owned(),
            r#"{"components": {"minecraft:enchantable": {"value": 14}}}"#.to_owned(),
        ]
    }

    #[test]
    fn an_enchantment_is_read_with_its_costs_its_sets_and_the_items_enchantability() {
        let lists = Lists::small();
        let compiled = compile(
            &[
                ("minecraft:sharpness".to_owned(), SHARPNESS.to_owned()),
                ("minecraft:mending".to_owned(), MENDING.to_owned()),
            ],
            &reports(),
            &lists.names(),
        )
        .unwrap();
        assert_eq!(
            compiled.enchantments,
            [
                Enchantment {
                    name: "minecraft:mending".to_owned(),
                    weight: 2,
                    max_level: 1,
                    min_cost: Cost {
                        base: 25,
                        per_level_above_first: 25
                    },
                    max_cost: Cost {
                        base: 75,
                        per_level_above_first: 25
                    },
                    exclusive_set: None,
                    supported_items: ItemSet::Tag("#minecraft:enchantable/durability".to_owned()),
                    primary_items: None,
                },
                Enchantment {
                    name: "minecraft:sharpness".to_owned(),
                    weight: 10,
                    max_level: 5,
                    min_cost: Cost {
                        base: 1,
                        per_level_above_first: 11
                    },
                    max_cost: Cost {
                        base: 21,
                        per_level_above_first: 11
                    },
                    exclusive_set: Some(NamedSet::Tag(
                        "#minecraft:exclusive_set/damage".to_owned()
                    )),
                    supported_items: ItemSet::Tag("#minecraft:swords".to_owned()),
                    primary_items: Some(ItemSet::Listed(vec![2])),
                },
            ]
        );
        assert_eq!(compiled.enchantable, [(1, 1), (2, 14)]);
        let text = emit(&compiled).unwrap();
        assert!(text.starts_with(
            "use crate::enchanting::{Enchantment, EnchantmentCost};\n\
             use crate::trading::{ItemSet, NamedSet};\n\n"
        ));
        assert!(text.contains(
            "    Enchantment { name: \"minecraft:sharpness\", weight: 10, max_level: 5, \
             min_cost: EnchantmentCost { base: 1, per_level_above_first: 11 }, max_cost: \
             EnchantmentCost { base: 21, per_level_above_first: 11 }, exclusive_set: \
             Some(NamedSet::Tag(\"#minecraft:exclusive_set/damage\")), supported_items: \
             ItemSet::Tag(\"#minecraft:swords\"), primary_items: Some(ItemSet::Listed(&[2])) },\n"
        ));
        assert!(text.ends_with(
            "pub static ENCHANTABLE: [(u16, i32); 2] = [\n    (1, 1),\n    (2, 14),\n];\n"
        ));
    }

    #[test]
    fn what_the_reader_cannot_place_fails_the_run() {
        let lists = Lists::small();
        let refused = |sharpness: String| {
            compile(
                &[
                    ("minecraft:mending".to_owned(), MENDING.to_owned()),
                    ("minecraft:sharpness".to_owned(), sharpness),
                ],
                &reports(),
                &lists.names(),
            )
            .is_err()
        };
        assert!(!refused(SHARPNESS.to_owned()));
        assert!(refused(
            SHARPNESS.replace("#minecraft:swords", "#minecraft:axes")
        ));
        assert!(refused(
            SHARPNESS.replace("minecraft:iron_sword", "minecraft:stick")
        ));
        assert!(refused(
            SHARPNESS.replace("exclusive_set/damage", "exclusive_set/armor")
        ));
        assert!(refused(SHARPNESS.replace(
            r#""base": 21, "per_level_above_first": 11"#,
            r#""type": "minecraft:clamped""#
        )));
        assert!(refused(
            SHARPNESS.replace(r#""weight": 10"#, r#""weight": 0"#)
        ));
        // An enchantment missing from the registry's entries.
        assert!(
            compile(
                &[("minecraft:mending".to_owned(), MENDING.to_owned())],
                &reports(),
                &lists.names()
            )
            .is_err()
        );
    }

    /// Reads every enchantment and item report of the data generator's output that
    /// `cargo datagen` keeps in `target/datagen`; needs no Java. Run it with
    /// `cargo test -p clustine-datagen -- --ignored generated_enchantments`; with
    /// `CLUSTINE_ENCHANTMENTS_OUT` set, it writes the emitted table there.
    #[test]
    #[ignore = "needs the data generator's output in target/datagen"]
    fn every_enchantment_of_the_generated_enchantments_is_understood() {
        use crate::model;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let generated = root
            .join("target/datagen")
            .join(crate::jar::SERVER_JAR.version)
            .join("generated");
        let data = generated.join("data/minecraft");
        let registries = model::read_json(&generated.join("reports/registries.json")).unwrap();
        let items = model::static_registry(&registries, "item").unwrap();
        let entries = |directory: &str, prefix: &str| -> Vec<String> {
            tables_under(&data.join(directory))
                .unwrap()
                .into_iter()
                .map(|path| format!("{prefix}minecraft:{path}"))
                .collect()
        };
        let enchantments = entries("enchantment", "");
        let item_tags = entries("tags/item", "#");
        let enchantment_tags = entries("tags/enchantment", "#");
        let compiled = load(
            &data,
            &generated.join("reports/minecraft/components/item"),
            &Names {
                items: &items,
                enchantments: &enchantments,
                item_tags: &item_tags,
                enchantment_tags: &enchantment_tags,
            },
        )
        .unwrap();
        assert_eq!(compiled.enchantments.len(), 43);
        assert_eq!(compiled.enchantable.len(), 77);
        let book = items
            .iter()
            .position(|item| item == "minecraft:book")
            .unwrap();
        assert!(compiled.enchantable.contains(&(book, 1)));
        let text = emit(&compiled).unwrap();
        println!("{} bytes", text.len());
        if let Some(out) = std::env::var_os("CLUSTINE_ENCHANTMENTS_OUT") {
            fs::write(out, text).unwrap();
        }
    }
}
