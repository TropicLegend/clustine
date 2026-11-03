//! The damage types (V4): every `data/minecraft/damage_type/<name>.json`, read by
//! `DamageType.CODEC` (n7 DamageType.lambda$static$0 @1-84) and rendered as the table
//! `clustine_data::DAMAGE_TYPES`, sorted by name.
//!
//! A field the codec does not know, or a value the game's enums do not have, is refused,
//! so that a game update that brings one stops the generator rather than being dropped.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

/// One damage type as its file gives it, with the codec's defaults where a field is not
/// written. The enums are the names of the variants in `clustine_data`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DamageTypeReport {
    pub name: String,
    pub message_id: String,
    pub scaling: &'static str,
    /// Float bits.
    pub exhaustion: u32,
    pub effects: &'static str,
    pub death_message_type: &'static str,
}

/// `DamageScaling`'s serialised names (n7 DamageScaling <clinit> @4-37), each with its
/// variant in `clustine_data`.
const SCALINGS: [(&str, &str); 3] = [
    ("never", "Never"),
    (
        "when_caused_by_living_non_player",
        "WhenCausedByLivingNonPlayer",
    ),
    ("always", "Always"),
];

/// `DamageEffects`'s serialised names (n7 DamageEffects <clinit> @4-97).
const EFFECTS: [(&str, &str); 6] = [
    ("hurt", "Hurt"),
    ("thorns", "Thorns"),
    ("drowning", "Drowning"),
    ("burning", "Burning"),
    ("poking", "Poking"),
    ("freezing", "Freezing"),
];

/// `DeathMessageType`'s serialised names (n7 DeathMessageType <clinit> @4-37).
const DEATH_MESSAGE_TYPES: [(&str, &str); 3] = [
    ("default", "Default"),
    ("fall_variants", "FallVariants"),
    ("intentional_game_design", "IntentionalGameDesign"),
];

/// Reads the damage types `entries` names (the synchronised registry's, which lists
/// every file of `dir`), sorted by name.
pub fn load(dir: &Path, entries: &[String]) -> Result<Vec<DamageTypeReport>> {
    let mut types = Vec::new();
    for name in entries {
        let local = name
            .strip_prefix("minecraft:")
            .with_context(|| format!("the damage type {name} is not the game's"))?;
        let path = dir.join(format!("{local}.json"));
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let json: Value =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        types.push(read(name, &json).with_context(|| format!("reading {}", path.display()))?);
    }
    types.sort_by(|a, b| a.name.cmp(&b.name));
    ensure!(
        types.windows(2).all(|pair| pair[0].name < pair[1].name),
        "two damage types of one name"
    );
    Ok(types)
}

/// `DamageType.CODEC` (n7 DamageType.lambda$static$0): `message_id` (@4), `scaling`
/// (@22) and `exhaustion` (@40) written; `effects` `hurt` (@58-60) and
/// `death_message_type` `default` (@79-81) where not.
pub fn read(name: &str, json: &Value) -> Result<DamageTypeReport> {
    const FIELDS: [&str; 5] = [
        "message_id",
        "scaling",
        "exhaustion",
        "effects",
        "death_message_type",
    ];
    let object = json.as_object().context("not a JSON object")?;
    if let Some(key) = object.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        bail!("a field {key} that is not understood");
    }
    let text = |key: &str| -> Result<Option<&str>> {
        object
            .get(key)
            .map(|value| {
                value
                    .as_str()
                    .with_context(|| format!("{key} that is not a string"))
            })
            .transpose()
    };
    let variant = |key: &str,
                   names: &[(&str, &'static str)],
                   default: Option<&str>|
     -> Result<&'static str> {
        let written = match (text(key)?, default) {
            (Some(written), _) => written,
            (None, Some(default)) => default,
            (None, None) => bail!("no {key}"),
        };
        Ok(names
            .iter()
            .find(|(known, _)| *known == written)
            .with_context(|| format!("a {key} {written} the game does not have"))?
            .1)
    };
    let exhaustion = object
        .get("exhaustion")
        .context("no exhaustion")?
        .as_f64()
        .context("an exhaustion that is not a number")? as f32;
    ensure!(exhaustion.is_finite(), "an exhaustion that is not finite");
    Ok(DamageTypeReport {
        name: name.to_owned(),
        message_id: text("message_id")?.context("no message_id")?.to_owned(),
        scaling: variant("scaling", &SCALINGS, None)?,
        exhaustion: exhaustion.to_bits(),
        effects: variant("effects", &EFFECTS, Some("hurt"))?,
        death_message_type: variant("death_message_type", &DEATH_MESSAGE_TYPES, Some("default"))?,
    })
}

/// `damage_types.rs`: the table, sorted by name for `clustine_data::damage_type`'s
/// binary search.
pub fn emit(types: &[DamageTypeReport]) -> Result<String> {
    ensure!(
        types.windows(2).all(|pair| pair[0].name < pair[1].name),
        "the damage types are not sorted by name"
    );
    let mut out = String::from(
        "use crate::{DamageEffects, DamageScaling, DamageTypeData, DeathMessageType};\n\n",
    );
    writeln!(out, "/// Sorted by name.")?;
    writeln!(
        out,
        "pub static DAMAGE_TYPES: [DamageTypeData; {}] = [",
        types.len()
    )?;
    for kind in types {
        let exhaustion = f32::from_bits(kind.exhaustion);
        writeln!(
            out,
            "    DamageTypeData {{ name: {:?}, message_id: {:?}, scaling: DamageScaling::{}, \
             exhaustion: {exhaustion:?}, effects: DamageEffects::{}, \
             death_message_type: DeathMessageType::{} }},",
            kind.name, kind.message_id, kind.scaling, kind.effects, kind.death_message_type
        )?;
    }
    writeln!(out, "];")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_damage_type_without_effects_or_a_death_message_type_takes_the_codecs_defaults() {
        let kind = read(
            "minecraft:generic",
            &json!({"message_id": "generic", "scaling": "when_caused_by_living_non_player",
                    "exhaustion": 0.0}),
        )
        .unwrap();
        assert_eq!(
            kind,
            DamageTypeReport {
                name: "minecraft:generic".to_owned(),
                message_id: "generic".to_owned(),
                scaling: "WhenCausedByLivingNonPlayer",
                exhaustion: 0.0_f32.to_bits(),
                effects: "Hurt",
                death_message_type: "Default",
            }
        );
    }

    #[test]
    fn every_field_written_is_read() {
        let kind = read(
            "minecraft:fall",
            &json!({"message_id": "fall", "scaling": "always", "exhaustion": 0.1,
                    "effects": "freezing", "death_message_type": "fall_variants"}),
        )
        .unwrap();
        assert_eq!(
            (
                kind.scaling,
                f32::from_bits(kind.exhaustion),
                kind.effects,
                kind.death_message_type
            ),
            ("Always", 0.1, "Freezing", "FallVariants")
        );
    }

    #[test]
    fn a_value_the_enums_do_not_have_or_a_field_not_understood_is_refused() {
        for text in [
            r#"{"message_id": "a", "scaling": "sometimes", "exhaustion": 0.0}"#,
            r#"{"message_id": "a", "scaling": "never", "exhaustion": 0.0, "effects": "tickling"}"#,
            r#"{"message_id": "a", "scaling": "never", "exhaustion": 0.0,
                "death_message_type": "poetic"}"#,
            r#"{"message_id": "a", "scaling": "never", "exhaustion": 0.0, "colour": 1}"#,
            r#"{"message_id": "a", "exhaustion": 0.0}"#,
            r#"{"scaling": "never", "exhaustion": 0.0}"#,
            r#"{"message_id": "a", "scaling": "never"}"#,
            r#"{"message_id": "a", "scaling": "never", "exhaustion": "much"}"#,
        ] {
            let json: Value = serde_json::from_str(text).unwrap();
            assert!(read("minecraft:a", &json).is_err(), "{text}");
        }
    }

    #[test]
    fn the_table_is_written_sorted_and_refused_otherwise() {
        let kind = |name: &str| DamageTypeReport {
            name: name.to_owned(),
            message_id: "inFire".to_owned(),
            scaling: "Never",
            exhaustion: 0.1_f32.to_bits(),
            effects: "Burning",
            death_message_type: "Default",
        };
        let out = emit(&[kind("minecraft:a"), kind("minecraft:b")]).unwrap();
        assert!(out.contains("pub static DAMAGE_TYPES: [DamageTypeData; 2] = [\n"));
        assert!(out.contains(
            "    DamageTypeData { name: \"minecraft:a\", message_id: \"inFire\", \
             scaling: DamageScaling::Never, exhaustion: 0.1, effects: DamageEffects::Burning, \
             death_message_type: DeathMessageType::Default },\n"
        ));
        assert!(emit(&[kind("minecraft:b"), kind("minecraft:a")]).is_err());
    }
}
