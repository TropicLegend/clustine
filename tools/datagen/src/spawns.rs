//! Emits `spawns.rs`: what each biome makes when mobs are made, from the attributes of
//! `worldgen/biome/**` (`docs/groundwork/n7-brief.md`, section 8.0 items 4 and 5, and
//! 8.9). The list is `minecraft:gameplay/natural_mob_spawns`, whose every biome overlays
//! the empty settings and so is the biome's own, by category; the probability of the
//! creatures made with a chunk is `minecraft:gameplay/creature_world_gen_spawn_probability`,
//! 0.1 where a biome does not set it.

use std::fmt::Write as _;

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;
use crate::worldgen::Worldgen;

/// `MobSpawnSettings.DEFAULT_CREATURE_WORLD_GEN_SPAWN_PROBABILITY`.
const DEFAULT_PROBABILITY: &str = "0.1";

/// The attributes this reads.
const SPAWNS: &str = "minecraft:gameplay/natural_mob_spawns";
const PROBABILITY: &str = "minecraft:gameplay/creature_world_gen_spawn_probability";

/// Emits the table, a row for each biome of `biomes` (the registry's order), each entity
/// type by its number among `entity_types`.
pub fn emit(biomes: &[String], entity_types: &[String], worldgen: &Worldgen) -> Result<String> {
    let of_biomes = worldgen.registry("biome")?;
    ensure!(
        of_biomes.len() == biomes.len(),
        "worldgen/biome has {} biomes and the registry {}",
        of_biomes.len(),
        biomes.len()
    );
    let kind = |name: &str| -> Result<usize> {
        entity_types
            .iter()
            .position(|known| known == name)
            .with_context(|| format!("{name} is no entity type"))
    };
    let mut out = String::new();
    writeln!(
        out,
        "//! What each biome makes when mobs are made, in the order of the biomes' numbers: \
         its"
    )?;
    writeln!(
        out,
        "//! spawners by category (a category it does not define is absent), its spawn \
         costs, and"
    )?;
    writeln!(
        out,
        "//! the probability of the creatures made with a chunk."
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "use crate::spawns::{{BiomeSpawns, SpawnCost, Spawner}};"
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "pub static BIOME_SPAWNS: [BiomeSpawns; {}] = [",
        biomes.len()
    )?;
    for biome in biomes {
        let json = of_biomes
            .get(biome)
            .with_context(|| format!("{biome} is in the registry and not in worldgen/biome"))?;
        let attributes = json.get("attributes");
        let probability = match attributes.and_then(|attributes| attributes.get(PROBABILITY)) {
            None => DEFAULT_PROBABILITY.to_owned(),
            Some(Json::Number(text)) => text.clone(),
            Some(other) => bail!("{biome}'s {PROBABILITY} is {other:?}, not a number"),
        };
        writeln!(out, "    BiomeSpawns {{")?;
        writeln!(out, "        name: {biome:?},")?;
        writeln!(
            out,
            "        creature_probability: {},",
            float(&probability)?
        )?;
        let spawns = attributes.and_then(|attributes| attributes.get(SPAWNS));
        let (categories, costs) = match spawns {
            None => (Vec::new(), Vec::new()),
            Some(spawns) => {
                let modifier = spawns.get("modifier").map(Json::string).transpose()?;
                ensure!(
                    modifier == Some("overlay"),
                    "{biome}'s {SPAWNS} has the modifier {modifier:?}, not overlay"
                );
                let argument = spawns.member("argument")?;
                let mut categories = Vec::new();
                if let Some(by) = argument.get("spawns_by_category") {
                    for (category, list) in by.members()? {
                        let mut spawners = Vec::new();
                        for entry in list.array()? {
                            let (min, max) = count(entry.member("count")?)
                                .with_context(|| format!("{biome}'s {category}"))?;
                            spawners.push(format!(
                                "Spawner {{ kind: {}, weight: {}, min_count: {min}, \
                                 max_count: {max} }}",
                                kind(entry.member("type")?.string()?)?,
                                entry.member("weight")?.integer()?
                            ));
                        }
                        categories.push((category.clone(), spawners));
                    }
                }
                let mut costs = Vec::new();
                if let Some(by) = argument.get("spawn_costs") {
                    for (name, cost) in by.members()? {
                        costs.push(format!(
                            "SpawnCost {{ kind: {}, energy_budget: {}, charge: {} }}",
                            kind(name)?,
                            double(cost.member("energy_budget")?)?,
                            double(cost.member("charge")?)?
                        ));
                    }
                }
                (categories, costs)
            }
        };
        writeln!(out, "        categories: &[")?;
        for (category, spawners) in categories {
            writeln!(out, "            ({category:?}, &[")?;
            for spawner in spawners {
                writeln!(out, "                {spawner},")?;
            }
            writeln!(out, "            ]),")?;
        }
        writeln!(out, "        ],")?;
        writeln!(out, "        costs: &[")?;
        for cost in costs {
            writeln!(out, "            {cost},")?;
        }
        writeln!(out, "        ],")?;
        writeln!(out, "    }},")?;
    }
    writeln!(out, "];")?;
    Ok(out)
}

/// A count: a constant, or `minecraft:uniform` with both ends included.
fn count(json: &Json) -> Result<(i32, i32)> {
    match json {
        Json::Number(_) => {
            let n = json.integer()?;
            Ok((n, n))
        }
        Json::Object(_) => {
            let kind = json.member("type")?.string()?;
            ensure!(kind == "minecraft:uniform", "a count of {kind}");
            Ok((
                json.member("min_inclusive")?.integer()?,
                json.member("max_inclusive")?.integer()?,
            ))
        }
        other => bail!("a count that is {other:?}"),
    }
}

/// A float as Rust writes it, from the file's text.
fn float(text: &str) -> Result<String> {
    let value: f32 = text
        .parse()
        .with_context(|| format!("{text} is no float"))?;
    Ok(format!("{value:?}_f32"))
}

/// A double as Rust writes it.
fn double(json: &Json) -> Result<String> {
    let Json::Number(text) = json else {
        bail!("{json:?} is no number");
    };
    let value: f64 = text
        .parse()
        .with_context(|| format!("{text} is no double"))?;
    Ok(format!("{value:?}"))
}
