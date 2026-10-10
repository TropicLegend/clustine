//! Values the game computes, committed for tests to compare with (ADR-0019, section 1,
//! row 15): the noise routers' entries at fixed positions.
//!
//! The extract program writes them; here they are read, checked for their shape and
//! written again in the same form, so that what is committed is what this reader
//! understands. A file names the noise settings and the seed, then each entry of the
//! router: `entry <name>` for one with a value in every row, `constant <name> <bits>`
//! for one whose value is the same at every position. A row is a position (x, y, z)
//! and the values of the entries in their order, each the bits of a `float` in
//! hexadecimal.

use std::fmt::Write;

use anyhow::{Context, Result, bail, ensure};

use crate::worldgen::{AQUIFER_ENTRIES, ROUTER_ENTRIES};

/// The values of one dimension's router for one seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterValues {
    /// The name of the noise settings: `minecraft:overworld`.
    pub settings: String,
    pub seed: i64,
    pub entries: Vec<Entry>,
    /// A position and the bits of the value of every entry that is not constant.
    pub rows: Vec<([i32; 3], Vec<u32>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The entry's name in the data; an entry of the aquifers has `aquifers.` before it.
    pub name: String,
    /// The bits of its value where it is the same at every position.
    pub constant: Option<u32>,
}

impl RouterValues {
    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        let mut header = |word: &str| -> Result<&str> {
            lines
                .next()
                .and_then(|line| line.strip_prefix(word))
                .and_then(|rest| rest.strip_prefix(' '))
                .with_context(|| format!("a line that starts with {word:?} is expected"))
        };
        let settings = header("settings")?.to_owned();
        let seed = header("seed")?
            .parse()
            .map_err(|_| anyhow::anyhow!("the seed is not a number"))?;

        let mut entries = Vec::new();
        let mut rows = Vec::new();
        for line in lines {
            let fields: Vec<&str> = line.split(' ').collect();
            match fields[..] {
                ["entry", name] => {
                    ensure!(rows.is_empty(), "an entry after the first row");
                    entries.push(Entry {
                        name: name.to_owned(),
                        constant: None,
                    });
                }
                ["constant", name, value] => {
                    ensure!(rows.is_empty(), "an entry after the first row");
                    entries.push(Entry {
                        name: name.to_owned(),
                        constant: Some(bits(value)?),
                    });
                }
                [x, y, z, ref values @ ..] => {
                    let varying = entries.iter().filter(|e| e.constant.is_none()).count();
                    ensure!(
                        values.len() == varying,
                        "a row has {} values for {varying} entries",
                        values.len()
                    );
                    let position = [coordinate(x)?, coordinate(y)?, coordinate(z)?];
                    let values = values
                        .iter()
                        .map(|v| bits(v))
                        .collect::<Result<Vec<u32>>>()?;
                    rows.push((position, values));
                }
                _ => bail!("the line {line:?} is neither an entry nor a row"),
            }
        }
        ensure!(!rows.is_empty(), "there are no rows");
        Ok(Self {
            settings,
            seed,
            entries,
            rows,
        })
    }

    /// Fails unless the entries are those of a router, in the game's order, followed
    /// by those of the aquifers if the dimension has them (`aquifers`).
    pub fn check_entries(&self, aquifers: bool) -> Result<()> {
        let mut expected: Vec<String> = ROUTER_ENTRIES.iter().map(|e| (*e).to_owned()).collect();
        if aquifers {
            expected.extend(AQUIFER_ENTRIES.iter().map(|e| format!("aquifers.{e}")));
        }
        let found: Vec<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        ensure!(
            found == expected,
            "the values of {} are of the entries {found:?}, the settings have {expected:?}",
            self.settings
        );
        Ok(())
    }

    /// The file's text.
    pub fn render(&self) -> String {
        let mut out = format!("settings {}\nseed {}\n", self.settings, self.seed);
        for entry in &self.entries {
            let _ = match entry.constant {
                Some(value) => writeln!(out, "constant {} {value:x}", entry.name),
                None => writeln!(out, "entry {}", entry.name),
            };
        }
        for ([x, y, z], values) in &self.rows {
            let _ = write!(out, "{x} {y} {z}");
            for value in values {
                let _ = write!(out, " {value:x}");
            }
            out.push('\n');
        }
        out
    }

    /// The values for a reader: a row a line, each value with its entry's name and as
    /// the number its bits are.
    pub fn dump(&self) -> String {
        let mut out = format!(
            "values of the noise router of {} for the seed {}, {} positions\n",
            self.settings,
            self.seed,
            self.rows.len()
        );
        for entry in &self.entries {
            if let Some(value) = entry.constant {
                let _ = writeln!(
                    out,
                    "{} = {:?} at every position",
                    entry.name,
                    f32::from_bits(value)
                );
            }
        }
        let varying: Vec<&str> = self
            .entries
            .iter()
            .filter(|entry| entry.constant.is_none())
            .map(|entry| entry.name.as_str())
            .collect();
        for ([x, y, z], values) in &self.rows {
            let _ = write!(out, "{x} {y} {z}");
            for (name, value) in varying.iter().zip(values) {
                let _ = write!(out, " {name}={:?}", f32::from_bits(*value));
            }
            out.push('\n');
        }
        out
    }
}

fn bits(text: &str) -> Result<u32> {
    ensure!(
        !text.is_empty()
            && text.len() <= 8
            && (text == "0" || !text.starts_with('0'))
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{text:?} is not the bits of a float in hexadecimal"
    );
    Ok(u32::from_str_radix(text, 16)?)
}

fn coordinate(text: &str) -> Result<i32> {
    let value: i32 = text
        .parse()
        .map_err(|_| anyhow::anyhow!("{text:?} is not a coordinate"))?;
    // Only one way of writing a number, so that reading and writing give the file.
    ensure!(value.to_string() == text, "{text:?} is not a coordinate");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "settings minecraft:nether\nseed -7\nentry temperature\n\
                        constant continents 0\nentry final_density\n\
                        0 0 0 3f000000 bf800000\n-5 64 29999984 80000000 3e19999a\n";

    #[test]
    fn a_file_is_read_and_written_back_as_it_was() {
        let values = RouterValues::parse(FILE).unwrap();
        assert_eq!(values.settings, "minecraft:nether");
        assert_eq!(values.seed, -7);
        assert_eq!(values.entries.len(), 3);
        assert_eq!(values.entries[1].constant, Some(0));
        assert_eq!(values.entries[2].name, "final_density");
        assert_eq!(values.rows[1].0, [-5, 64, 29_999_984]);
        assert_eq!(values.rows[1].1, [0x8000_0000, 0x3e19_999a]);
        assert_eq!(values.render(), FILE);
    }

    #[test]
    fn a_file_of_another_shape_is_refused() {
        for (what, text) in [
            ("no settings", "seed 1\nentry a\n0 0 0 0\n"),
            ("no rows", "settings s\nseed 1\nentry a\n"),
            (
                "a value too few",
                "settings s\nseed 1\nentry a\nentry b\n0 0 0 1\n",
            ),
            (
                "a value too many",
                "settings s\nseed 1\nconstant a 1\n0 0 0 1\n",
            ),
            (
                "an entry after a row",
                "settings s\nseed 1\nentry a\n0 0 0 1\nentry b\n",
            ),
            (
                "upper case",
                "settings s\nseed 1\nentry a\n0 0 0 3F000000\n",
            ),
            ("leading zeros", "settings s\nseed 1\nentry a\n0 0 0 01\n"),
            (
                "nine digits",
                "settings s\nseed 1\nentry a\n0 0 0 123456789\n",
            ),
            (
                "a coordinate with a plus",
                "settings s\nseed 1\nentry a\n+1 0 0 1\n",
            ),
            ("a short row", "settings s\nseed 1\n0 0\n"),
        ] {
            assert!(RouterValues::parse(text).is_err(), "{what}");
        }
    }

    #[test]
    fn the_entries_have_to_be_the_routers_in_the_games_order() {
        let mut text = "settings s\nseed 1\n".to_owned();
        for entry in ROUTER_ENTRIES {
            text.push_str(&format!("constant {entry} 0\n"));
        }
        let without = RouterValues::parse(&format!("{text}0 0 0\n")).unwrap();
        assert!(without.check_entries(false).is_ok());
        assert!(without.check_entries(true).is_err());
        for entry in AQUIFER_ENTRIES {
            text.push_str(&format!("constant aquifers.{entry} 0\n"));
        }
        let with = RouterValues::parse(&format!("{text}0 0 0\n")).unwrap();
        assert!(with.check_entries(true).is_ok());
        assert!(with.check_entries(false).is_err());
        assert!(
            RouterValues::parse(FILE)
                .unwrap()
                .check_entries(false)
                .is_err()
        );
    }

    #[test]
    fn the_dump_names_each_value_and_writes_it_as_a_number() {
        let dump = RouterValues::parse(FILE).unwrap().dump();
        assert_eq!(
            dump,
            "values of the noise router of minecraft:nether for the seed -7, 2 positions\n\
             continents = 0.0 at every position\n\
             0 0 0 temperature=0.5 final_density=-1.0\n\
             -5 64 29999984 temperature=-0.0 final_density=0.15\n"
        );
    }
}
