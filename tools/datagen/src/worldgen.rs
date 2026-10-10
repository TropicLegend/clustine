//! The game's world-generation data, read from the entries of the game's own jar
//! (`data/minecraft/worldgen/**`), which is what a server loads (ADR-0019, section 3).
//!
//! Every registry is kept by name, sorted, which is the order ADR-0019 commits the
//! registries in that are reached by name only. A reference from one entry to another
//! is resolved here, at datagen time: one that names nothing fails the run.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::density::Density;
use crate::jar;
use crate::json::Json;
use crate::model::Block;

/// Where the registries are in the game's jar.
const PREFIX: &str = "data/minecraft/worldgen/";

/// The registries whose every file the data generator has to have written with the
/// same bytes as the jar holds: the reference values of the routers are computed from
/// the game's built-in registries, which the generator writes out, while the emitter
/// reads the jar's files. Equal bytes say that the two are the same data.
const CHECKED_AGAINST_THE_GENERATOR: [&str; 3] = ["density_function", "noise", "noise_settings"];

pub struct Worldgen {
    /// By registry (`density_function`), then by entry (`minecraft:overworld/depth`).
    registries: BTreeMap<String, BTreeMap<String, Json>>,
}

impl Worldgen {
    /// Reads the registries from the verified bundle `jar`. `generated` is the data
    /// generator's output; a file both have has to be the same in both.
    pub fn load(jar: &Path, generated: &Path) -> Result<Self> {
        let game_jar = jar::game_jar(jar)?;
        let files = jar::files_below(&game_jar, PREFIX)?;
        ensure!(
            !files.is_empty(),
            "the game's jar has nothing below {PREFIX}"
        );

        let written = generated.join(PREFIX);
        for (path, bytes) in &files {
            let registry = path.split('/').next().unwrap_or_default();
            match fs::read(written.join(path)) {
                Ok(other) => ensure!(
                    other == *bytes,
                    "{PREFIX}{path} of the jar is not what the jar's data generator writes"
                ),
                Err(_) => ensure!(
                    !CHECKED_AGAINST_THE_GENERATOR.contains(&registry),
                    "the data generator did not write {PREFIX}{path}"
                ),
            }
        }
        Self::from_files(&files)
    }

    /// The registries from files by their path below `worldgen/`.
    pub fn from_files(files: &BTreeMap<String, Vec<u8>>) -> Result<Self> {
        let mut registries: BTreeMap<String, BTreeMap<String, Json>> = BTreeMap::new();
        for (path, bytes) in files {
            let (registry, rest) = path
                .split_once('/')
                .with_context(|| format!("{PREFIX}{path} is in no registry"))?;
            let name = rest
                .strip_suffix(".json")
                .with_context(|| format!("{PREFIX}{path} is not a JSON file"))?;
            let text = std::str::from_utf8(bytes)
                .with_context(|| format!("{PREFIX}{path} is not UTF-8"))?;
            let json = Json::parse(text).with_context(|| format!("reading {PREFIX}{path}"))?;
            registries
                .entry(registry.to_owned())
                .or_default()
                .insert(format!("minecraft:{name}"), json);
        }
        Ok(Self { registries })
    }

    /// The entries of a registry by name, sorted.
    pub fn registry(&self, name: &str) -> Result<&BTreeMap<String, Json>> {
        self.registries
            .get(name)
            .with_context(|| format!("the game's data has no registry worldgen/{name}"))
    }

    /// One entry, which has to be there.
    pub fn entry(&self, registry: &str, name: &str) -> Result<&Json> {
        self.registry(registry)?
            .get(name)
            .with_context(|| format!("{name} is referred to and is not in worldgen/{registry}"))
    }
}

/// The parameters of one noise (`worldgen/noise`).
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseParameters {
    pub base_octave: i32,
    pub base_amplitude: f64,
    pub octave_count: i32,
    pub normalise: bool,
    /// One for each octave, or none where every octave counts in full.
    pub amplitude_modifiers: Vec<f64>,
}

impl NoiseParameters {
    pub fn parse(json: &Json) -> Result<Self> {
        for (key, _) in json.members()? {
            ensure!(
                [
                    "base_octave",
                    "base_amplitude",
                    "octave_count",
                    "normalize",
                    "amplitude_modifiers"
                ]
                .contains(&key.as_str()),
                "the member {key:?} of a noise is not known to the emitter"
            );
        }
        let octave_count = match json.get("octave_count") {
            Some(count) => count.integer()?,
            None => 1,
        };
        let amplitude_modifiers = match json.get("amplitude_modifiers") {
            Some(list) => list
                .array()?
                .iter()
                .map(Json::double)
                .collect::<Result<Vec<f64>>>()?,
            None => Vec::new(),
        };
        ensure!(octave_count >= 1, "{octave_count} octaves");
        ensure!(
            amplitude_modifiers.is_empty() || amplitude_modifiers.len() == octave_count as usize,
            "{} amplitude modifiers for {octave_count} octaves",
            amplitude_modifiers.len()
        );
        Ok(Self {
            base_octave: json.member("base_octave")?.integer()?,
            base_amplitude: match json.get("base_amplitude") {
                Some(amplitude) => amplitude.double()?,
                None => 1.0,
            },
            octave_count,
            normalise: match json.get("normalize") {
                Some(normalise) => normalise.boolean()?,
                None => true,
            },
            amplitude_modifiers,
        })
    }
}

/// The entries of a noise router in the order of the game's record, and those of the
/// aquifers' settings likewise. They are the members' names in the data.
pub const ROUTER_ENTRIES: [&str; 8] = [
    "temperature",
    "vegetation",
    "continents",
    "erosion",
    "depth",
    "ridges",
    "chunk_surface_level",
    "final_density",
];
pub const AQUIFER_ENTRIES: [&str; 6] = [
    "barrier",
    "fluid_level_floodedness",
    "fluid_level_spread",
    "lava",
    "exclusion",
    "surface_level",
];

/// One of `worldgen/noise_settings`: what a dimension's terrain is made by.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseSettings {
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    pub legacy_random_source: bool,
    pub disable_mob_generation: bool,
    /// State ids.
    pub default_block: u64,
    pub default_fluid: u64,
    /// The name of the material rule, which is there in `worldgen/material_rule`.
    pub material_rule: String,
    /// In the order of [`ROUTER_ENTRIES`].
    pub router: Vec<Density>,
    /// In the order of [`AQUIFER_ENTRIES`], where the dimension has aquifers.
    pub aquifers: Option<Vec<Density>>,
    /// Each a set of ranges that a place to start in has to lie within: a density
    /// function's name with its lowest and highest value.
    pub spawn_target: Vec<Vec<(String, f32, f32)>>,
}

impl NoiseSettings {
    pub fn parse(json: &Json, worldgen: &Worldgen, blocks: &[Block]) -> Result<Self> {
        for (key, _) in json.members()? {
            ensure!(
                [
                    "aquifers",
                    "debug_functions",
                    "default_block",
                    "default_fluid",
                    "disable_mob_generation",
                    "legacy_random_source",
                    "material_rule",
                    "noise",
                    "noise_router",
                    "sea_level",
                    "spawn_target",
                ]
                .contains(&key.as_str()),
                "the member {key:?} of noise settings is not known to the emitter"
            );
        }
        let entries = |object: &Json, names: &[&str], what: &str| -> Result<Vec<Density>> {
            for (key, _) in object.members()? {
                ensure!(
                    names.contains(&key.as_str()),
                    "the entry {key:?} of the {what} is not known to the emitter"
                );
            }
            names
                .iter()
                .map(|name| {
                    Density::parse(object.member(name)?)
                        .with_context(|| format!("reading {name} of the {what}"))
                })
                .collect()
        };

        let noise = json.member("noise")?;
        let material_rule = json.member("material_rule")?.string()?.to_owned();
        worldgen.entry("material_rule", &material_rule)?;

        let mut spawn_target = Vec::new();
        for point in json.member("spawn_target")?.array()? {
            let mut ranges = Vec::new();
            for (function, range) in point.members()? {
                let [min, max] = range.array()? else {
                    bail!("a range of the spawn target does not have two ends");
                };
                worldgen.entry("density_function", function)?;
                ranges.push((function.clone(), min.float()?, max.float()?));
            }
            spawn_target.push(ranges);
        }

        Ok(Self {
            min_y: noise.member("min_y")?.integer()?,
            height: noise.member("height")?.integer()?,
            sea_level: json.member("sea_level")?.integer()?,
            legacy_random_source: json.member("legacy_random_source")?.boolean()?,
            disable_mob_generation: json.member("disable_mob_generation")?.boolean()?,
            default_block: block_state(json.member("default_block")?, blocks)
                .context("reading the default block")?,
            default_fluid: block_state(json.member("default_fluid")?, blocks)
                .context("reading the default fluid")?,
            material_rule,
            router: entries(
                json.member("noise_router")?,
                &ROUTER_ENTRIES,
                "noise router",
            )?,
            aquifers: match json.get("aquifers") {
                Some(aquifers) => Some(entries(aquifers, &AQUIFER_ENTRIES, "aquifers")?),
                None => None,
            },
            spawn_target,
        })
    }
}

/// The state id of a block state as `worldgen/**` writes one: a bare name (the block's
/// default state), `{id}` (the same), or `{id, properties}` with a value for every
/// property. It is resolved against `blocks`, the model of this run, and a name or a
/// value the model does not have fails.
pub fn block_state(json: &Json, blocks: &[Block]) -> Result<u64> {
    let find = |name: &str| {
        blocks
            .iter()
            .find(|block| block.name == name)
            .with_context(|| format!("{name} is no block"))
    };
    let (block, properties) = match json {
        Json::String(name) => return Ok(find(name)?.default_state),
        Json::Object(members) => {
            for (key, _) in members {
                ensure!(
                    key == "id" || key == "properties",
                    "the member {key:?} of a block state is not known to the emitter"
                );
            }
            let block = find(json.member("id")?.string()?)?;
            match json.get("properties") {
                Some(properties) => (block, properties.members()?),
                None => return Ok(block.default_state),
            }
        }
        other => bail!("a block state cannot be {other:?}"),
    };
    for (name, _) in properties {
        ensure!(
            block
                .properties
                .iter()
                .any(|property| property.name == *name),
            "{} has no property {name}",
            block.name
        );
    }
    let mut index = 0;
    for property in &block.properties {
        let value = properties
            .iter()
            .find(|(name, _)| *name == property.name)
            .with_context(|| format!("no value is given for {} of {}", property.name, block.name))?
            .1
            .string()?;
        let place = property
            .values
            .iter()
            .position(|known| known == value)
            .with_context(|| {
                format!("{value} is no value of {} of {}", property.name, block.name)
            })?;
        index = index * property.values.len() + place;
    }
    Ok(block.first_state + index as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Property;

    fn files(entries: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
            .collect()
    }

    fn blocks() -> Vec<Block> {
        let property = |name: &str, values: &[&str]| Property {
            name: name.to_owned(),
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        };
        vec![
            Block {
                name: "minecraft:stone".to_owned(),
                first_state: 1,
                last_state: 1,
                default_state: 1,
                properties: Vec::new(),
            },
            Block {
                name: "minecraft:slab".to_owned(),
                first_state: 10,
                last_state: 15,
                default_state: 13,
                properties: vec![
                    property("type", &["top", "bottom", "double"]),
                    property("waterlogged", &["true", "false"]),
                ],
            },
        ]
    }

    #[test]
    fn registries_are_kept_by_name_with_the_directories_in_the_name() {
        let worldgen = Worldgen::from_files(&files(&[
            ("noise/erosion.json", r#"{"base_octave": -9}"#),
            ("density_function/overworld/caves/noodle.json", "1.0"),
            ("density_function/zero.json", "0.0"),
        ]))
        .unwrap();
        let names: Vec<&String> = worldgen
            .registry("density_function")
            .unwrap()
            .keys()
            .collect();
        assert_eq!(
            names,
            ["minecraft:overworld/caves/noodle", "minecraft:zero"]
        );
        assert!(worldgen.entry("noise", "minecraft:erosion").is_ok());
        let missing = worldgen.entry("noise", "minecraft:nothing").unwrap_err();
        assert!(format!("{missing}").contains("minecraft:nothing is referred to"));
        assert!(worldgen.registry("biome").is_err());
        assert!(Worldgen::from_files(&files(&[("noise/a.txt", "1")])).is_err());
        assert!(Worldgen::from_files(&files(&[("noise/a.json", "{")])).is_err());
    }

    #[test]
    fn a_noise_has_one_octave_and_is_normalised_unless_it_says_otherwise() {
        let parse = |text: &str| NoiseParameters::parse(&Json::parse(text).unwrap());
        assert_eq!(
            parse(r#"{"base_amplitude": 0.5, "base_octave": -3}"#).unwrap(),
            NoiseParameters {
                base_octave: -3,
                base_amplitude: 0.5,
                octave_count: 1,
                normalise: true,
                amplitude_modifiers: Vec::new(),
            }
        );
        let full = parse(
            r#"{"amplitude_modifiers": [1.0, 0.0], "base_amplitude": 1.25, "base_octave": -9,
                "octave_count": 2, "normalize": false}"#,
        )
        .unwrap();
        assert_eq!(full.amplitude_modifiers, [1.0, 0.0]);
        assert!(!full.normalise);
        assert!(parse(r#"{"base_octave": -3, "amplitude_modifiers": [1.0, 1.0]}"#).is_err());
        assert!(parse(r#"{"base_octave": -3, "firstOctave": 1}"#).is_err());
        assert!(parse(r#"{"base_amplitude": 1.0}"#).is_err());
    }

    #[test]
    fn a_block_state_is_resolved_in_each_way_the_data_writes_one() {
        let blocks = blocks();
        let state = |text: &str| block_state(&Json::parse(text).unwrap(), &blocks);
        assert_eq!(state("\"minecraft:slab\"").unwrap(), 13);
        assert_eq!(state(r#"{"id": "minecraft:slab"}"#).unwrap(), 13);
        assert_eq!(
            state(r#"{"id": "minecraft:slab", "properties": {"waterlogged": "true", "type": "double"}}"#)
                .unwrap(),
            14
        );
        assert_eq!(
            state(r#"{"id": "minecraft:stone", "properties": {}}"#).unwrap(),
            1
        );
    }

    #[test]
    fn a_block_state_that_the_model_does_not_have_fails_in_each_way_of_writing_it() {
        let blocks = blocks();
        let state = |text: &str| block_state(&Json::parse(text).unwrap(), &blocks);
        for text in [
            "\"minecraft:nothing\"",
            r#"{"id": "minecraft:nothing"}"#,
            r#"{"id": "minecraft:nothing", "properties": {}}"#,
            r#"{"id": "minecraft:slab", "properties": {"type": "sideways", "waterlogged": "true"}}"#,
            r#"{"id": "minecraft:slab", "properties": {"type": "top"}}"#,
            r#"{"id": "minecraft:slab", "properties": {"type": "top", "waterlogged": "true", "lit": "true"}}"#,
            r#"{"Name": "minecraft:stone"}"#,
            "7",
        ] {
            assert!(state(text).is_err(), "{text}");
        }
    }

    fn settings_text(extra: &str) -> String {
        format!(
            r#"{{"default_block": "minecraft:stone", "default_fluid": {{"id": "minecraft:slab"}},
                "disable_mob_generation": false, "legacy_random_source": true,
                "material_rule": "minecraft:rule", "noise": {{"height": 128, "min_y": 0}},
                "noise_router": {{"chunk_surface_level": 0.0, "continents": 0.0, "depth": 0.0,
                    "erosion": "minecraft:zero", "final_density": 1.5, "ridges": 0.0,
                    "temperature": 0.0, "vegetation": 0.0}},
                "sea_level": 32, "debug_functions": [],
                "spawn_target": [{{"minecraft:zero": [-0.11, 1.0]}}]{extra}}}"#
        )
    }

    fn small_worldgen() -> Worldgen {
        Worldgen::from_files(&files(&[
            ("density_function/zero.json", "0.0"),
            ("material_rule/rule.json", "{}"),
        ]))
        .unwrap()
    }

    #[test]
    fn noise_settings_give_the_router_in_the_records_order() {
        let worldgen = small_worldgen();
        let json = Json::parse(&settings_text("")).unwrap();
        let settings = NoiseSettings::parse(&json, &worldgen, &blocks()).unwrap();
        assert_eq!(
            (settings.min_y, settings.height, settings.sea_level),
            (0, 128, 32)
        );
        assert!(settings.legacy_random_source && !settings.disable_mob_generation);
        assert_eq!((settings.default_block, settings.default_fluid), (1, 13));
        assert_eq!(settings.router.len(), 8);
        assert_eq!(
            settings.router[3],
            Density::Reference("minecraft:zero".to_owned())
        );
        assert_eq!(settings.router[7], Density::Constant(1.5));
        assert!(settings.aquifers.is_none());
        assert_eq!(
            settings.spawn_target,
            [vec![("minecraft:zero".to_owned(), -0.11_f32, 1.0_f32)]]
        );
    }

    #[test]
    fn noise_settings_with_an_entry_the_emitter_does_not_know_fail() {
        let worldgen = small_worldgen();
        let parse =
            |text: &str| NoiseSettings::parse(&Json::parse(text).unwrap(), &worldgen, &blocks());
        assert!(parse(&settings_text(r#", "ore_veins_enabled": true"#)).is_err());
        let with_vein =
            settings_text("").replace(r#""ridges": 0.0,"#, r#""ridges": 0.0, "vein_gap": 0.0,"#);
        assert!(parse(&with_vein).is_err());
        let no_rule = settings_text("").replace("minecraft:rule", "minecraft:no_rule");
        assert!(format!("{:#}", parse(&no_rule).unwrap_err()).contains("minecraft:no_rule"));
        let no_block = settings_text("").replace("minecraft:stone", "minecraft:no_block");
        assert!(parse(&no_block).is_err());
        let no_function = settings_text("").replace(
            r#"{"minecraft:zero": [-0.11, 1.0]}"#,
            r#"{"minecraft:one": [0, 1]}"#,
        );
        assert!(parse(&no_function).is_err());
        let aquifers = settings_text(
            r#", "aquifers": {"barrier": 0, "exclusion": 0, "fluid_level_floodedness": 0,
                "fluid_level_spread": 0, "lava": 0, "surface_level": 0}"#,
        );
        assert_eq!(parse(&aquifers).unwrap().aquifers.unwrap().len(), 6);
    }
}
