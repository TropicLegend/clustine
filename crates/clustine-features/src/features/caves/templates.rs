//! What the two features built of structure templates share (`fossil` and
//! `template`): their processor lists read through `clustine-structures`, the lists
//! the data names, and the name of a template checked against the table of templates.
//!
//! The data names processor lists of the registry `worldgen/processor_list`, which
//! `cargo datagen` does not emit for features. The three that features name (the
//! fossils') are written out here as the jar's files have them;
//! `the_processor_lists_written_here_are_the_jars` holds them to the files.

use clustine_structures::processors::ProcessorList;
use clustine_worldgen_data::feature::Data;

use crate::reader::{DataError, Read, Reader};

/// `minecraft:fossil_rot`.
static FOSSIL_ROT: Data = Data::Object(&[(
    "processors",
    Data::List(&[
        Data::Object(&[
            ("integrity", Data::Number("0.9")),
            ("processor_type", Data::Text("minecraft:block_rot")),
        ]),
        Data::Object(&[
            ("processor_type", Data::Text("minecraft:protected_blocks")),
            ("value", Data::Text("#minecraft:features_cannot_replace")),
        ]),
    ]),
)]);

/// `minecraft:fossil_coal`.
static FOSSIL_COAL: Data = Data::Object(&[(
    "processors",
    Data::List(&[
        Data::Object(&[
            ("integrity", Data::Number("0.1")),
            ("processor_type", Data::Text("minecraft:block_rot")),
        ]),
        Data::Object(&[
            ("processor_type", Data::Text("minecraft:protected_blocks")),
            ("value", Data::Text("#minecraft:features_cannot_replace")),
        ]),
    ]),
)]);

/// `minecraft:fossil_diamonds`.
static FOSSIL_DIAMONDS: Data = Data::Object(&[(
    "processors",
    Data::List(&[
        Data::Object(&[
            ("integrity", Data::Number("0.1")),
            ("processor_type", Data::Text("minecraft:block_rot")),
        ]),
        Data::Object(&[
            ("processor_type", Data::Text("minecraft:rule")),
            (
                "rules",
                Data::List(&[Data::Object(&[
                    (
                        "input_predicate",
                        Data::Object(&[
                            ("block", Data::Text("minecraft:coal_ore")),
                            ("predicate_type", Data::Text("minecraft:block_match")),
                        ]),
                    ),
                    (
                        "location_predicate",
                        Data::Object(&[("predicate_type", Data::Text("minecraft:always_true"))]),
                    ),
                    (
                        "output_state",
                        Data::Text("minecraft:deepslate_diamond_ore"),
                    ),
                ])]),
            ),
        ]),
        Data::Object(&[
            ("processor_type", Data::Text("minecraft:protected_blocks")),
            ("value", Data::Text("#minecraft:features_cannot_replace")),
        ]),
    ]),
)]);

/// The processor lists the features' data names, by name.
pub static NAMED_LISTS: [(&str, &Data); 3] = [
    ("minecraft:fossil_coal", &FOSSIL_COAL),
    ("minecraft:fossil_diamonds", &FOSSIL_DIAMONDS),
    ("minecraft:fossil_rot", &FOSSIL_ROT),
];

fn named_list(name: &str) -> Option<&'static Data> {
    NAMED_LISTS
        .iter()
        .find(|(listed, _)| *listed == name)
        .map(|(_, data)| *data)
}

/// Reads a processor list where the game's codec takes a `Holder` of one: the name of
/// a list, or a list in place. The list's own members are checked by
/// `clustine-structures`' reader, as this crate's checks a feature's.
pub fn read_processors(reader: &Reader) -> Read<ProcessorList> {
    let inner = clustine_structures::reader::Reader::new(reader.data(), reader.at());
    ProcessorList::read(&inner, &named_list).map_err(|error| DataError {
        at: error.at,
        what: error.what,
    })
}

/// Reads a template's name and checks that the jar has it, by the table of templates
/// `cargo datagen` made: a name the table does not know is a data error, not a
/// template missing at a feature's first placing.
pub fn read_template_name(reader: &Reader) -> Read<&'static str> {
    let name = reader.text()?;
    let plain = name.strip_prefix("minecraft:").unwrap_or(name);
    let table = clustine_worldgen_data::templates();
    match table.find(plain) {
        Some(index) if table.get(index).exists => Ok(name),
        _ => Err(reader.error(format!("{name} is not a template of the jar"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The data as the jar's file writes it, without white space: members in the
    /// order written, which is the files' sorted order.
    fn json(data: &Data) -> String {
        match data {
            Data::Bool(value) => value.to_string(),
            Data::Number(number) => (*number).to_string(),
            Data::Text(text) => format!("\"{text}\""),
            Data::List(items) => {
                let items: Vec<String> = items.iter().map(json).collect();
                format!("[{}]", items.join(","))
            }
            Data::Object(members) => {
                let members: Vec<String> = members
                    .iter()
                    .map(|(key, value)| format!("\"{key}\":{}", json(value)))
                    .collect();
                format!("{{{}}}", members.join(","))
            }
        }
    }

    #[test]
    fn every_named_list_reads_whole() {
        for (name, data) in NAMED_LISTS {
            let list = read_processors(&Reader::new(data, name)).unwrap();
            assert!(!list.processors.is_empty(), "{name}");
        }
        assert!(NAMED_LISTS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        static NAMED: Data = Data::Text("minecraft:fossil_rot");
        assert_eq!(
            read_processors(&Reader::new(&NAMED, "test")).unwrap(),
            read_processors(&Reader::new(&FOSSIL_ROT, "test")).unwrap()
        );
        static UNKNOWN: Data = Data::Text("minecraft:no_such_list");
        assert!(read_processors(&Reader::new(&UNKNOWN, "test")).is_err());
    }

    #[test]
    fn a_template_the_jar_does_not_have_is_refused() {
        static WELL: Data = Data::Text("minecraft:desert_well/well");
        static NONE: Data = Data::Text("minecraft:desert_well/no_such_well");
        assert_eq!(
            read_template_name(&Reader::new(&WELL, "test")),
            Ok("minecraft:desert_well/well")
        );
        assert!(read_template_name(&Reader::new(&NONE, "test")).is_err());
    }

    /// The lists written here against the jar's files, which `cargo datagen` extracts.
    /// `CLUSTINE_DATAGEN` names its output: `target/datagen/26.3/generated`.
    #[test]
    #[ignore = "needs the files `cargo datagen` extracts, named by CLUSTINE_DATAGEN"]
    fn the_processor_lists_written_here_are_the_jars() {
        let root = std::env::var("CLUSTINE_DATAGEN").expect("CLUSTINE_DATAGEN names the output");
        for (name, data) in NAMED_LISTS {
            let file = name.strip_prefix("minecraft:").unwrap();
            let path = format!("{root}/data/minecraft/worldgen/processor_list/{file}.json");
            let text = std::fs::read_to_string(&path).unwrap();
            let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            assert_eq!(json(data), compact, "{name}");
        }
    }
}
