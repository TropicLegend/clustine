//! `cargo datagen --dump <table>`: a packed table as text, one row a line, so that a
//! version update can be read (ADR-0019, section 1).
//!
//! It reads the committed file and needs no jar. The names of blocks and biomes are
//! not in a packed table; they are read from the generated Rust beside it in the
//! repository, and a row is printed with its number alone where they cannot be.
//!
//! `git diff` can be given this as a `textconv`; see `.gitattributes`.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::packed::{self, Parsed, ParsedSection};
use crate::tables::{FLAGS, PARAMETER_LISTS, PUSH_REACTIONS};

const DATA_DIR: &str = "crates/clustine-data/src/generated";

/// The text of the table `table`: the name of a committed table (`block_states`,
/// `biome_parameters`) or the path of a file.
pub fn run(root: &Path, table: &str) -> Result<String> {
    let path = match table {
        "block_states" | "biome_parameters" => root.join(DATA_DIR).join(format!("{table}.bin")),
        path => PathBuf::from(path),
    };
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let generated = root.join(DATA_DIR);
    let read = |name: &str| fs::read_to_string(generated.join(name)).unwrap_or_default();
    render(
        &bytes,
        &block_ranges(&read("blocks.rs")),
        &biome_names(&read("registries.rs")),
    )
}

/// The text of the packed table `bytes`. `blocks` are the blocks with their first and
/// last state; `biomes` the biomes by Clustine's ids. Either may be empty.
pub fn render(bytes: &[u8], blocks: &[(String, u32, u32)], biomes: &[String]) -> Result<String> {
    let table = packed::parse(bytes)?;
    let sha1: String = table
        .sha1
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut out = format!(
        "packed table CLT1, kind {}, layout {}, jar {sha1}, {} sections\n",
        table.kind,
        table.layout,
        table.sections.len()
    );
    match table.kind {
        packed::KIND_BLOCK_STATES => block_states(&table, blocks, &mut out)?,
        packed::KIND_BIOME_PARAMETERS => biome_parameters(&table, biomes, &mut out)?,
        other => bail!("this datagen does not know tables of kind {other}"),
    }
    Ok(out)
}

fn expect_rows(table: &Parsed<'_>, row_bytes: &[usize]) -> Result<()> {
    let found: Vec<usize> = table.sections.iter().map(|s| s.row_bytes).collect();
    ensure!(
        found == row_bytes,
        "the table has sections with rows of {found:?} bytes, this layout has {row_bytes:?}"
    );
    Ok(())
}

fn block_states(table: &Parsed<'_>, blocks: &[(String, u32, u32)], out: &mut String) -> Result<()> {
    expect_rows(table, &[16, 12, 4, 32, 4, 48, 3])?;
    let [
        states,
        sets,
        face_starts,
        rectangles,
        collision_starts,
        boxes,
        offsets,
    ] = &table.sections[..]
    else {
        bail!("the table does not have seven sections");
    };

    writeln!(out, "[states: {} rows]", states.rows().len())?;
    let mut block = 0;
    for (id, row) in states.rows().enumerate() {
        while blocks
            .get(block)
            .is_some_and(|(_, _, last)| (*last as usize) < id)
        {
            block += 1;
        }
        let name = match blocks.get(block) {
            Some((name, first, _)) => format!("{name}+{}", id - *first as usize),
            None => "?".to_owned(),
        };
        let flags = u16::from_le_bytes([row[2], row[3]]);
        let set: Vec<&str> = FLAGS
            .iter()
            .enumerate()
            .filter(|(bit, _)| flags & (1 << bit) != 0)
            .map(|(_, name)| *name)
            .collect();
        write!(
            out,
            "{id} {name} emission={} dampening={} flags={} sturdy={}/{}/{} push={}",
            row[0],
            row[1],
            if set.is_empty() {
                "-".to_owned()
            } else {
                set.join(",")
            },
            faces(row[4]),
            faces(row[5]),
            faces(row[6]),
            PUSH_REACTIONS
                .get(usize::from(row[7]))
                .copied()
                .unwrap_or("?"),
        )?;
        match row[8] {
            0 => write!(out, " fluid=-")?,
            fluid => write!(
                out,
                " fluid={}:{}{}{}",
                fluid - 1,
                row[9] & 0x3f,
                if row[9] & 0x40 != 0 { ":source" } else { "" },
                if row[9] & 0x80 != 0 { ":falling" } else { "" },
            )?,
        }
        match row[10] {
            0 => write!(out, " post=-")?,
            1 => write!(out, " post=0,0,0")?,
            index => {
                let offset = offsets.row(usize::from(index) - 2)?;
                write!(
                    out,
                    " post={},{},{}",
                    offset[0] as i8, offset[1] as i8, offset[2] as i8
                )?;
            }
        }
        writeln!(
            out,
            " faces={} collision={}",
            u16::from_le_bytes([row[12], row[13]]),
            u16::from_le_bytes([row[14], row[15]])
        )?;
    }

    writeln!(out, "[occlusion face sets: {} rows]", sets.rows().len())?;
    for (index, row) in sets.rows().enumerate() {
        let shapes: Vec<String> = row
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]).to_string())
            .collect();
        writeln!(out, "faces {index}: {}", shapes.join(" "))?;
    }
    shapes(out, "face shape", face_starts, rectangles)?;
    shapes(out, "collision shape", collision_starts, boxes)?;
    writeln!(
        out,
        "[post-processing offsets: {} rows]",
        offsets.rows().len()
    )?;
    for (index, row) in offsets.rows().enumerate() {
        writeln!(
            out,
            "post {}: {},{},{}",
            index + 2,
            row[0] as i8,
            row[1] as i8,
            row[2] as i8
        )?;
    }
    Ok(())
}

/// Six faces as the letters of those that are set: down, up, north, south, west, east.
fn faces(bits: u8) -> String {
    let letters: String = "DUNSWE"
        .chars()
        .enumerate()
        .filter(|(bit, _)| bits & (1 << bit) != 0)
        .map(|(_, letter)| letter)
        .collect();
    if letters.is_empty() {
        "-".to_owned()
    } else {
        letters
    }
}

/// Shapes as a section of starts and a section of rows of doubles.
fn shapes(
    out: &mut String,
    what: &str,
    starts: &ParsedSection<'_>,
    rows: &ParsedSection<'_>,
) -> Result<()> {
    let starts: Vec<usize> = starts
        .rows()
        .map(|row| u32::from_le_bytes([row[0], row[1], row[2], row[3]]) as usize)
        .collect();
    writeln!(
        out,
        "[{what}s: {} shapes, {} rows]",
        starts.len().saturating_sub(1),
        rows.rows().len()
    )?;
    for (index, range) in starts.windows(2).enumerate() {
        write!(out, "{what} {index}:")?;
        for row in range[0]..range[1] {
            let numbers: Vec<String> = rows
                .row(row)?
                .chunks(8)
                .map(|bits| {
                    let mut eight = [0; 8];
                    eight.copy_from_slice(bits);
                    f64::from_le_bytes(eight).to_string()
                })
                .collect();
            write!(out, " [{}]", numbers.join(","))?;
        }
        writeln!(out)?;
    }
    Ok(())
}

fn biome_parameters(table: &Parsed<'_>, biomes: &[String], out: &mut String) -> Result<()> {
    expect_rows(table, &[27; PARAMETER_LISTS.len()])?;
    for (name, section) in PARAMETER_LISTS.iter().zip(&table.sections) {
        writeln!(out, "[{name}: {} rows]", section.rows().len())?;
        for (index, row) in section.rows().enumerate() {
            let numbers: Vec<i16> = row[..26]
                .chunks(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            let bounds: Vec<String> = numbers[..12]
                .chunks(2)
                .map(|pair| format!("{}..{}", pair[0], pair[1]))
                .collect();
            let biome = biomes.get(usize::from(row[26])).map_or("?", String::as_str);
            writeln!(
                out,
                "{name} {index} {biome}({}) {} offset={}",
                row[26],
                bounds.join(" "),
                numbers[12]
            )?;
        }
    }
    Ok(())
}

/// The blocks of the generated `blocks.rs` with their first and last state.
fn block_ranges(blocks_rs: &str) -> Vec<(String, u32, u32)> {
    let mut blocks = Vec::new();
    for line in blocks_rs.lines() {
        let Some(rest) = line.trim().strip_prefix("Block { name: \"") else {
            continue;
        };
        let Some((name, rest)) = rest.split_once('"') else {
            continue;
        };
        let mut states = rest
            .split("BlockState(")
            .skip(1)
            .filter_map(|part| part.split(')').next()?.parse::<u32>().ok());
        if let (Some(first), Some(last)) = (states.next(), states.next()) {
            blocks.push((name.to_owned(), first, last));
        }
    }
    blocks
}

/// The entries of the registry `minecraft:worldgen/biome` in the generated
/// `registries.rs`.
fn biome_names(registries_rs: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in registries_rs.lines() {
        let line = line.trim();
        if line.starts_with("name: ") {
            inside = line == "name: \"minecraft:worldgen/biome\",";
        } else if inside && line.starts_with('"') {
            names.push(line.trim_matches(|c| c == '"' || c == ',').to_owned());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packed::Section;

    const SHA1: &str = "33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c";

    #[test]
    fn names_are_read_from_the_generated_rust() {
        let blocks = block_ranges(
            "pub static BLOCKS: [Block; 2] = [\n    Block { name: \"minecraft:air\", first_state: \
             BlockState(0), last_state: BlockState(0), default_state: BlockState(0) },\n    Block \
             { name: \"minecraft:grass_block\", first_state: BlockState(8), last_state: \
             BlockState(9), default_state: BlockState(9) },\n];\n",
        );
        assert_eq!(
            blocks,
            [
                ("minecraft:air".to_owned(), 0, 0),
                ("minecraft:grass_block".to_owned(), 8, 9)
            ]
        );
        let biomes = biome_names(
            "    Registry {\n        name: \"minecraft:worldgen/biome\",\n        entries: &[\n   \
             \"minecraft:badlands\",\n            \"minecraft:plains\",\n        ],\n    },\n    \
             Registry {\n        name: \"minecraft:chat_type\",\n        entries: &[\n            \
             \"minecraft:chat\",\n        ],\n    },\n",
        );
        assert_eq!(biomes, ["minecraft:badlands", "minecraft:plains"]);
    }

    #[test]
    fn a_table_of_parameters_is_printed_a_row_a_line_with_the_biomes_name() {
        let mut overworld = Section::new(27);
        let mut row = Vec::new();
        for number in [-10000i16, 10000, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 3750] {
            row.extend_from_slice(&number.to_le_bytes());
        }
        row.push(1);
        overworld.push(&row);
        let bytes = packed::file(
            packed::KIND_BIOME_PARAMETERS,
            1,
            SHA1,
            &[overworld, Section::new(27)],
        )
        .unwrap();
        let biomes = [
            "minecraft:badlands".to_owned(),
            "minecraft:plains".to_owned(),
        ];
        let text = render(&bytes, &[], &biomes).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains(SHA1), "{}", lines[0]);
        assert_eq!(lines[1], "[overworld: 1 rows]");
        assert_eq!(
            lines[2],
            "overworld 0 minecraft:plains(1) -10000..10000 1..2 3..4 5..6 7..8 9..10 offset=3750"
        );
        assert_eq!(lines[3], "[nether: 0 rows]");
    }

    #[test]
    fn what_is_no_table_or_of_an_unknown_kind_is_refused() {
        assert!(render(b"not a table", &[], &[]).is_err());
        let bytes = packed::file(99, 1, SHA1, &[]).unwrap();
        assert!(render(&bytes, &[], &[]).is_err());
    }
}
