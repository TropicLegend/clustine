// Written for Clustine from the game's data (`worldgen/carver/**`, the member
// `carvers` of `worldgen/biome/**`) and the codecs of the pinned jar's classes that
// read it (`CaveWorldCarver`, `CanyonWorldCarver`, `CanyonWorldCarver$Shape`,
// `FloatProviders`, `IntProviders`, `UniformHeight`, `VerticalAnchor`). It uses the text
// helpers of the router's emitter (src/router.rs), which is adapted from SteelMC;
// nothing else here is.

//! Emits the carvers of `clustine-worldgen-data` and which biome has which.
//!
//! A carver of 26.3 is a record of numbers and of providers of numbers; the four of
//! the game are written as statics of the types of `clustine_worldgen_data::carver`,
//! sorted by name. A biome lists its carvers in an order that is part of each carver's
//! seed, so the list is written in the data's order, for the biomes in the order of
//! their numbers. A member, a kind of carver or a kind of provider the emitter does
//! not know fails the run, so that nothing the data says is passed over.

use std::fmt::Write;

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;
use crate::router::{LINE_WIDTH, check_layout, float};
use crate::worldgen::Worldgen;

/// How far a field of a carver is indented.
const FIELD: &str = "            ";

/// The members `json` may have beside its `type`.
fn only(json: &Json, what: &str, known: &[&str]) -> Result<()> {
    for (key, _) in json.members()? {
        ensure!(
            key == "type" || known.contains(&key.as_str()),
            "the member {key:?} of {what} is not known to the emitter"
        );
    }
    Ok(())
}

/// A height against the terrain's, as Rust.
fn anchor(json: &Json) -> Result<String> {
    let [(kind, value)] = json.members()? else {
        bail!("a height has one member");
    };
    let value = value.integer()?;
    Ok(match kind.as_str() {
        "absolute" => format!("Anchor::Absolute({value})"),
        "above_bottom" => format!("Anchor::AboveBottom({value})"),
        "below_top" => format!("Anchor::BelowTop({value})"),
        other => bail!("a height that is {other:?} is not known to the emitter"),
    })
}

/// A provider of heights, as Rust: only the uniform one.
fn height(json: &Json) -> Result<String> {
    let kind = json.member("type")?.string()?;
    ensure!(
        kind == "minecraft:uniform",
        "a provider of heights that is {kind:?} is not known to the emitter"
    );
    only(
        json,
        "a uniform height",
        &["min_inclusive", "max_inclusive"],
    )?;
    // Over several lines, as a field of a carver stands: on one it is too long.
    Ok(format!(
        "UniformHeight {{\n{FIELD}    min_inclusive: {},\n{FIELD}    max_inclusive: {},\n{FIELD}}}",
        anchor(json.member("min_inclusive")?)?,
        anchor(json.member("max_inclusive")?)?
    ))
}

/// A provider of floats, as Rust.
fn float_provider(json: &Json) -> Result<String> {
    if !matches!(json, Json::Object(_)) {
        return Ok(format!(
            "FloatProvider::Constant({})",
            float(json.float()?)?
        ));
    }
    let kind = json.member("type")?.string()?;
    Ok(match kind {
        "minecraft:constant" => {
            only(json, "a constant float", &["value"])?;
            format!(
                "FloatProvider::Constant({})",
                float(json.member("value")?.float()?)?
            )
        }
        "minecraft:uniform" => {
            only(json, "a uniform float", &["min_inclusive", "max_exclusive"])?;
            format!(
                "FloatProvider::Uniform {{ min_inclusive: {}, max_exclusive: {} }}",
                float(json.member("min_inclusive")?.float()?)?,
                float(json.member("max_exclusive")?.float()?)?
            )
        }
        "minecraft:trapezoid" => {
            only(json, "a trapezoid", &["min", "max", "plateau"])?;
            format!(
                "FloatProvider::Trapezoid {{ min: {}, max: {}, plateau: {} }}",
                float(json.member("min")?.float()?)?,
                float(json.member("max")?.float()?)?,
                float(json.member("plateau")?.float()?)?
            )
        }
        other => bail!("a provider of floats that is {other:?} is not known to the emitter"),
    })
}

/// A provider of ints, as Rust: only the kind the carvers use.
fn int_provider(json: &Json) -> Result<String> {
    let kind = json.member("type")?.string()?;
    ensure!(
        kind == "minecraft:very_biased_to_bottom",
        "a provider of ints that is {kind:?} is not known to the emitter"
    );
    only(
        json,
        "an int that is very biased to the bottom",
        &["min_inclusive", "max_inclusive"],
    )?;
    let (min, max) = (
        json.member("min_inclusive")?.integer()?,
        json.member("max_inclusive")?.integer()?,
    );
    ensure!(min <= max, "an int from {min} to {max}");
    Ok(format!(
        "IntProvider::VeryBiasedToBottom {{ min_inclusive: {min}, max_inclusive: {max} }}"
    ))
}

/// The fields of one carver, each a line.
fn carver(json: &Json) -> Result<(&'static str, Vec<(&'static str, String)>)> {
    let kind = json.member("type")?.string()?;
    let probability = |json: &Json| -> Result<String> {
        let probability = json.member("probability")?.float()?;
        ensure!(
            (0.0..=1.0).contains(&probability),
            "the probability {probability}"
        );
        float(probability)
    };
    match kind {
        "minecraft:cave" => {
            only(
                json,
                "a cave",
                &[
                    "probability",
                    "y",
                    "count",
                    "thickness",
                    "weird_thickness_bias",
                    "room_vertical_radius_multiplier",
                    "horizontal_radius_multiplier",
                    "vertical_radius_multiplier",
                    "start_vertical_radius_multiplier",
                    "floor_level",
                ],
            )?;
            let provider = |name: &str| -> Result<String> {
                float_provider(json.member(name)?).with_context(|| format!("reading {name}"))
            };
            Ok((
                "Cave",
                vec![
                    ("probability", probability(json)?),
                    ("y", height(json.member("y")?)?),
                    ("count", int_provider(json.member("count")?)?),
                    ("thickness", provider("thickness")?),
                    // The codec's default.
                    (
                        "weird_thickness_bias",
                        match json.get("weird_thickness_bias") {
                            Some(bias) => bias.boolean()?,
                            None => false,
                        }
                        .to_string(),
                    ),
                    (
                        "room_vertical_radius_multiplier",
                        provider("room_vertical_radius_multiplier")?,
                    ),
                    (
                        "horizontal_radius_multiplier",
                        provider("horizontal_radius_multiplier")?,
                    ),
                    (
                        "vertical_radius_multiplier",
                        provider("vertical_radius_multiplier")?,
                    ),
                    // The codec's default.
                    (
                        "start_vertical_radius_multiplier",
                        match json.get("start_vertical_radius_multiplier") {
                            Some(multiplier) => float_provider(multiplier)?,
                            None => "FloatProvider::Constant(1.0)".to_owned(),
                        },
                    ),
                    ("floor_level", provider("floor_level")?),
                ],
            ))
        }
        "minecraft:canyon" => {
            only(
                json,
                "a canyon",
                &["probability", "y", "vertical_rotation", "shape"],
            )?;
            let shape = json.member("shape")?;
            only(
                shape,
                "a canyon's shape",
                &[
                    "distance_factor",
                    "thickness",
                    "width_smoothness",
                    "horizontal_radius_factor",
                    "vertical_radius_default_factor",
                    "vertical_radius_center_factor",
                    "y_scale",
                ],
            )?;
            let of_shape = |name: &str| -> Result<String> {
                float_provider(shape.member(name)?).with_context(|| format!("reading {name}"))
            };
            let width_smoothness = shape.member("width_smoothness")?.integer()?;
            ensure!(
                width_smoothness >= 1,
                "a width smoothness of {width_smoothness}"
            );
            Ok((
                "Canyon",
                vec![
                    ("probability", probability(json)?),
                    ("y", height(json.member("y")?)?),
                    (
                        "vertical_rotation",
                        float_provider(json.member("vertical_rotation")?)?,
                    ),
                    ("distance_factor", of_shape("distance_factor")?),
                    ("thickness", of_shape("thickness")?),
                    ("width_smoothness", width_smoothness.to_string()),
                    (
                        "horizontal_radius_factor",
                        of_shape("horizontal_radius_factor")?,
                    ),
                    (
                        "vertical_radius_default_factor",
                        float(shape.member("vertical_radius_default_factor")?.float()?)?,
                    ),
                    (
                        "vertical_radius_center_factor",
                        float(shape.member("vertical_radius_center_factor")?.float()?)?,
                    ),
                    ("y_scale", of_shape("y_scale")?),
                ],
            ))
        }
        other => bail!("a carver that is {other:?} is not known to the emitter"),
    }
}

/// Emits `carvers.rs`: the carvers of `worldgen/carver`, sorted by name, and for each
/// biome, in the order of `biomes` (the biomes' numbers), the carvers it has in the
/// order its data lists them.
pub fn emit(biomes: &[String], worldgen: &Worldgen) -> Result<String> {
    let carvers = worldgen.registry("carver")?;
    let of_biomes = worldgen.registry("biome")?;
    ensure!(
        of_biomes.len() == biomes.len(),
        "worldgen/biome has {} biomes and the registry of the reports {}",
        of_biomes.len(),
        biomes.len()
    );

    let mut out = String::new();
    writeln!(
        out,
        "//! The game's carvers of caves and ravines (`worldgen/carver`), sorted by name, and \
         which"
    )?;
    writeln!(
        out,
        "//! of them each biome has: in the order of the biomes' numbers, each list in the \
         order of"
    )?;
    writeln!(
        out,
        "//! the biome's data, which is part of a carver's seed."
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "use crate::carver::{{Canyon, Carver, Cave, FloatProvider, IntProvider, Kind, \
         UniformHeight}};"
    )?;
    writeln!(out, "use crate::material::Anchor;")?;
    writeln!(out)?;
    writeln!(out, "pub static CARVERS: [Carver; {}] = [", carvers.len())?;
    for (name, json) in carvers {
        let (kind, fields) = carver(json).with_context(|| format!("reading the carver {name}"))?;
        writeln!(out, "    Carver {{")?;
        writeln!(out, "        name: {name:?},")?;
        writeln!(out, "        kind: Kind::{kind}({kind} {{")?;
        for (field, value) in fields {
            let line = format!("{FIELD}{field}: {value},");
            if line.lines().all(|line| line.len() <= LINE_WIDTH) {
                writeln!(out, "{line}")?;
            } else {
                writeln!(out, "{FIELD}{field}:\n{FIELD}    {value},")?;
            }
        }
        writeln!(out, "        }}),")?;
        writeln!(out, "    }},")?;
    }
    writeln!(out, "];")?;
    writeln!(out)?;

    let names: Vec<&String> = carvers.keys().collect();
    writeln!(out, "pub static OF_BIOME: [&[usize]; {}] = [", biomes.len())?;
    for biome in biomes {
        let json = of_biomes
            .get(biome)
            .with_context(|| format!("{biome} is in the registry and not in worldgen/biome"))?;
        // One carver is written as its name, several as a list.
        let listed = json.member("carvers")?;
        let listed: Vec<&str> = match listed {
            Json::String(name) => vec![name.as_str()],
            other => other
                .array()?
                .iter()
                .map(Json::string)
                .collect::<Result<Vec<&str>>>()?,
        };
        let mut numbers = Vec::new();
        for carver in listed {
            let number = names
                .iter()
                .position(|name| *name == carver)
                .with_context(|| format!("{biome} has the carver {carver}, which is none"))?;
            numbers.push(number.to_string());
        }
        writeln!(out, "    &[{}], // {biome}", numbers.join(", "))?;
    }
    writeln!(out, "];")?;
    check_layout(&out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const CAVE: &str = r#"{"type": "minecraft:cave",
        "count": {"type": "minecraft:very_biased_to_bottom", "max_inclusive": 9, "min_inclusive": 0},
        "floor_level": -0.7, "horizontal_radius_multiplier": 1.0, "probability": 0.2,
        "room_vertical_radius_multiplier": {"type": "minecraft:uniform", "max_exclusive": 0.9,
            "min_inclusive": 0.1},
        "thickness": {"type": "minecraft:trapezoid", "max": 6.0, "min": 0.0, "plateau": 2.0},
        "vertical_radius_multiplier": 1.0,
        "y": {"type": "minecraft:uniform", "max_inclusive": {"below_top": 1},
            "min_inclusive": {"absolute": 0}}}"#;

    const CANYON: &str = r#"{"type": "minecraft:canyon", "probability": 0.01,
        "shape": {"distance_factor": 0.75, "horizontal_radius_factor": 1.0,
            "thickness": 2.0, "vertical_radius_center_factor": 0.0,
            "vertical_radius_default_factor": 1.0, "width_smoothness": 3, "y_scale": 3.0},
        "vertical_rotation": 0.0,
        "y": {"type": "minecraft:uniform", "max_inclusive": {"absolute": 67},
            "min_inclusive": {"above_bottom": 10}}}"#;

    fn worldgen(carvers: &[(&str, &str)], biomes: &[(&str, &str)]) -> Worldgen {
        let mut files = BTreeMap::new();
        for (name, text) in carvers {
            files.insert(format!("carver/{name}.json"), text.as_bytes().to_vec());
        }
        for (name, carvers) in biomes {
            files.insert(
                format!("biome/{name}.json"),
                format!(r#"{{"carvers": {carvers}}}"#).into_bytes(),
            );
        }
        Worldgen::from_files(&files).unwrap()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names
            .iter()
            .map(|name| format!("minecraft:{name}"))
            .collect()
    }

    #[test]
    fn carvers_are_written_by_name_and_a_biomes_list_in_the_datas_order() {
        let worldgen = worldgen(
            &[("pit", CAVE), ("crack", CANYON)],
            &[
                ("hills", r#"["minecraft:pit", "minecraft:crack"]"#),
                ("sea", r#""minecraft:pit""#),
                ("sky", "[]"),
            ],
        );
        let text = emit(&names(&["sky", "hills", "sea"]), &worldgen).unwrap();
        let crack = text.find("name: \"minecraft:crack\"").unwrap();
        let pit = text.find("name: \"minecraft:pit\"").unwrap();
        assert!(crack < pit);
        assert!(text.contains("pub static CARVERS: [Carver; 2] = ["));
        assert!(text.contains("kind: Kind::Canyon(Canyon {"));
        assert!(text.contains(
            "            y: UniformHeight {\n                min_inclusive: Anchor::Absolute(0),\n                \
             max_inclusive: Anchor::BelowTop(1),\n            },\n"
        ));
        assert!(text.contains(
            "            room_vertical_radius_multiplier:\n                \
             FloatProvider::Uniform { min_inclusive: 0.1, max_exclusive: 0.9 },\n"
        ));
        assert!(
            text.contains(
                "thickness: FloatProvider::Trapezoid { min: 0.0, max: 6.0, plateau: 2.0 },"
            )
        );
        assert!(text.contains("floor_level: FloatProvider::Constant(-0.7),"));
        // What the data leaves out has the codec's default.
        assert!(text.contains("weird_thickness_bias: false,"));
        assert!(text.contains("start_vertical_radius_multiplier: FloatProvider::Constant(1.0),"));
        let lists = text.split("pub static OF_BIOME").nth(1).unwrap();
        assert!(lists.contains("    &[], // minecraft:sky\n    &[1, 0], // minecraft:hills\n    &[1], // minecraft:sea\n"));
    }

    #[test]
    fn two_runs_over_the_same_data_give_the_same_bytes() {
        let worldgen = worldgen(&[("pit", CAVE)], &[("sea", r#""minecraft:pit""#)]);
        let biomes = names(&["sea"]);
        assert_eq!(
            emit(&biomes, &worldgen).unwrap(),
            emit(&biomes, &worldgen).unwrap()
        );
    }

    #[test]
    fn what_the_emitter_does_not_know_fails_it() {
        let biomes = names(&["sea"]);
        let fails = |carver: &str, of_biome: &str| {
            emit(&biomes, &worldgen(&[("pit", carver)], &[("sea", of_biome)])).is_err()
        };
        let pit = r#""minecraft:pit""#;
        assert!(!fails(CAVE, pit));
        assert!(fails(&CAVE.replace("minecraft:cave", "minecraft:pit"), pit));
        assert!(fails(
            &CAVE.replace(
                "\"probability\": 0.2",
                "\"probability\": 0.2, \"lava_level\": 1"
            ),
            pit
        ));
        assert!(fails(
            &CAVE.replace("very_biased_to_bottom", "uniform"),
            pit
        ));
        assert!(fails(
            &CAVE.replace("minecraft:trapezoid", "minecraft:clamped_normal"),
            pit
        ));
        assert!(fails(&CAVE.replace("below_top", "below_sea"), pit));
        assert!(fails(
            &CAVE.replace("\"probability\": 0.2", "\"probability\": 1.5"),
            pit
        ));
        assert!(fails(
            &CANYON.replace("\"width_smoothness\": 3", "\"width_smoothness\": 0"),
            pit
        ));
        assert!(fails(
            &CANYON.replace("\"y_scale\": 3.0", "\"y_scale\": 3.0, \"depth\": 1"),
            pit
        ));
        // A biome that names a carver that is none, and a registry with a biome more.
        assert!(fails(CAVE, r#""minecraft:crack""#));
        assert!(
            emit(
                &names(&["sea", "sky"]),
                &worldgen(&[("pit", CAVE)], &[("sea", pit)])
            )
            .is_err()
        );
    }
}
