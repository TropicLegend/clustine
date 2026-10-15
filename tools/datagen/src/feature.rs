// Written for Clustine from the game's data (`worldgen/feature/**`,
// `worldgen/placed_feature/**`, `worldgen/block_state_provider/**`, the member
// `features` of `worldgen/biome/**`) and the order the game's own generators place
// features in, which `java/Extract.java` reads from them.

//! Emits the features of `clustine-worldgen-data`: every configured feature, placed
//! feature and named block-state provider as a tree that mirrors its file, sorted by
//! name; for each dimension the game's order of features by step; and for each biome
//! the placed features it lists in each step.
//!
//! The trees are not typed here, since only the place where a name stands says what it
//! names (`clustine_worldgen_data::feature`); the code that reads a type of feature reads
//! every member it is given. What is checked here is what can be without knowing the
//! types: that every feature a biome lists and every feature the game orders is a placed
//! feature of the data, that a biome lists a feature in a step the game places it in
//! where the game places it at all, and that the game orders no feature twice in a
//! dimension.

use std::collections::BTreeMap;
use std::fmt::Write;

use anyhow::{Context, Result, bail, ensure};

use crate::extract::FeatureOrder;
use crate::json::Json;
use crate::router::check_layout;
use crate::worldgen::Worldgen;

/// How many steps of decoration the game has (`GenerationStep.Decoration`).
const STEPS: usize = 11;

/// The emitted file `features.rs`.
pub fn emit(biomes: &[String], worldgen: &Worldgen, order: &FeatureOrder) -> Result<String> {
    let configured = worldgen.registry("feature")?;
    let placed = worldgen.registry("placed_feature")?;
    let providers = worldgen.registry("block_state_provider")?;
    let index: BTreeMap<&str, usize> = placed
        .keys()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    ensure!(
        placed.len() <= usize::from(u16::MAX),
        "the placed features are too many to be numbered by a u16"
    );
    let number = |name: &str, what: &dyn Fn() -> String| -> Result<u16> {
        let found = index
            .get(name)
            .with_context(|| format!("{} names {name}, which is not a placed feature", what()))?;
        Ok(u16::try_from(*found).expect("the count was checked"))
    };

    let mut out = String::new();
    writeln!(
        out,
        "//! Every configured feature, placed feature and named block-state provider, sorted by"
    )?;
    writeln!(
        out,
        "//! name; the order of each dimension's features by step, as the game's generators"
    )?;
    writeln!(
        out,
        "//! have it; and what each biome lists by step, in the order of the biomes' numbers."
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "use crate::feature::{{BiomeFeatures, Data::*, Entry, Order}};"
    )?;
    for (name, registry) in [
        ("CONFIGURED", configured),
        ("PLACED", placed),
        ("PROVIDERS", providers),
    ] {
        writeln!(out)?;
        writeln!(out, "pub static {name}: [Entry; {}] = [", registry.len())?;
        for (entry, json) in registry {
            writeln!(out, "    Entry {{")?;
            writeln!(out, "        name: {entry:?},")?;
            write!(out, "        data: ")?;
            data(&mut out, json, 2).with_context(|| format!("writing {entry}"))?;
            writeln!(out, ",")?;
            writeln!(out, "    }},")?;
        }
        writeln!(out, "];")?;
    }

    writeln!(out)?;
    writeln!(
        out,
        "pub static ORDER: [Order; {}] = [",
        order.dimensions.len()
    )?;
    for (dimension, steps) in &order.dimensions {
        ensure!(
            steps.len() <= STEPS,
            "the game orders {dimension}'s features in {} steps",
            steps.len()
        );
        let mut seen = Vec::new();
        let mut numbered = Vec::new();
        for (step, features) in steps.iter().enumerate() {
            let mut list = Vec::new();
            for feature in features {
                let found = number(feature, &|| {
                    format!("the order of {dimension}, step {step},")
                })?;
                ensure!(
                    !seen.contains(&found),
                    "the game orders {feature} twice in {dimension}"
                );
                seen.push(found);
                list.push(found);
            }
            numbered.push(list);
        }
        writeln!(out, "    Order {{")?;
        writeln!(out, "        dimension: {dimension:?},")?;
        steps_of(&mut out, &numbered)?;
        writeln!(out, "    }},")?;
    }
    writeln!(out, "];")?;

    let registry = worldgen.registry("biome")?;
    writeln!(out)?;
    writeln!(
        out,
        "pub static BIOME_FEATURES: [BiomeFeatures; {}] = [",
        biomes.len()
    )?;
    for biome in biomes {
        let json = registry
            .get(biome)
            .with_context(|| format!("the biome {biome} has no file"))?;
        let lists = json.member("features")?.array()?;
        ensure!(
            lists.len() <= STEPS,
            "{biome} lists features in {} steps",
            lists.len()
        );
        let mut numbered = Vec::new();
        for (step, features) in lists.iter().enumerate() {
            let mut list = Vec::new();
            for feature in features.array()? {
                let name = feature.string().with_context(|| {
                    format!("{biome} lists a feature in step {step} that is not a name")
                })?;
                let found = number(name, &|| format!("{biome}, in step {step},"))?;
                // A feature no dimension orders is one of a biome no dimension of a new
                // world has (the void of the flat presets): kept, and never placed.
                let ordered_in: Vec<usize> = order
                    .dimensions
                    .values()
                    .flat_map(|steps| {
                        steps
                            .iter()
                            .enumerate()
                            .filter(|(_, ordered)| ordered.iter().any(|known| known == name))
                            .map(|(at, _)| at)
                    })
                    .collect();
                // One placed feature can stand in another step in another dimension
                // (`spring_lava`: 8 in the overworld, 9 in the Nether).
                ensure!(
                    ordered_in.is_empty() || ordered_in.contains(&step),
                    "{biome} lists {name} in step {step}, where the game places it in {ordered_in:?}"
                );
                list.push(found);
            }
            numbered.push(list);
        }
        writeln!(out, "    BiomeFeatures {{")?;
        writeln!(out, "        biome: {biome:?},")?;
        steps_of(&mut out, &numbered)?;
        writeln!(out, "    }},")?;
    }
    writeln!(out, "];")?;
    check_layout(&out)?;
    Ok(out)
}

/// Writes a member `steps` of lists of numbers.
fn steps_of(out: &mut String, steps: &[Vec<u16>]) -> Result<()> {
    writeln!(out, "        steps: &[")?;
    for step in steps {
        if step.is_empty() {
            writeln!(out, "            &[],")?;
            continue;
        }
        writeln!(out, "            &[")?;
        let mut line = String::new();
        for number in step {
            let item = format!("{number},");
            if line.len() + item.len() + 1 > 100 - 16 {
                writeln!(out, "                {}", line.trim_end())?;
                line.clear();
            }
            line.push_str(&item);
            line.push(' ');
        }
        writeln!(out, "                {}", line.trim_end())?;
        writeln!(out, "            ],")?;
    }
    writeln!(out, "        ],")?;
    Ok(())
}

/// Writes `json` as a `Data` at the indentation of `depth` levels, without a newline at
/// the end.
fn data(out: &mut String, json: &Json, depth: usize) -> Result<()> {
    let inner = "    ".repeat(depth + 1);
    let closing = "    ".repeat(depth);
    match json {
        Json::Null => bail!("the data has a null, which no codec of a feature reads"),
        Json::Bool(value) => write!(out, "Bool({value})")?,
        Json::Number(text) => {
            ensure!(
                text.parse::<f64>().is_ok_and(f64::is_finite),
                "{text} is not a number"
            );
            write!(out, "Number({text:?})")?;
        }
        Json::String(text) => write!(out, "Text({text:?})")?,
        Json::Array(items) if items.is_empty() => write!(out, "List(&[])")?,
        Json::Array(items) => {
            writeln!(out, "List(&[")?;
            for item in items {
                write!(out, "{inner}")?;
                data(out, item, depth + 1)?;
                writeln!(out, ",")?;
            }
            write!(out, "{closing}])")?;
        }
        Json::Object(members) if members.is_empty() => write!(out, "Object(&[])")?,
        Json::Object(members) => {
            writeln!(out, "Object(&[")?;
            for (key, value) in members {
                write!(out, "{inner}({key:?}, ")?;
                data(out, value, depth + 1)?;
                writeln!(out, "),")?;
            }
            write!(out, "{closing}])")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_is_written_as_nested_data_in_the_files_order() {
        let json = Json::parse(
            r#"{"type": "minecraft:count", "count": 10, "flag": true, "list": [1.5, "a"], "empty": []}"#,
        )
        .unwrap();
        let mut out = String::new();
        data(&mut out, &json, 0).unwrap();
        assert_eq!(
            out,
            "Object(&[\n    (\"type\", Text(\"minecraft:count\")),\n    (\"count\", Number(\"10\")),\n    \
             (\"flag\", Bool(true)),\n    (\"list\", List(&[\n        Number(\"1.5\"),\n        \
             Text(\"a\"),\n    ])),\n    (\"empty\", List(&[])),\n])"
        );
    }

    #[test]
    fn a_null_is_refused() {
        let mut out = String::new();
        assert!(data(&mut out, &Json::Null, 0).is_err());
    }
}
