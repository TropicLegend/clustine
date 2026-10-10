//! Reads what the extract program (`tools/datagen/java/Extract.java`) wrote.
//!
//! The program writes text files, one row a line with the fields separated by tabs.
//! Integers are decimal and a double is its bits in hexadecimal, so that nothing rests
//! on how a JVM prints a fraction.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::reference::RouterValues;

/// A box as the bits of six doubles: the lower corner, then the upper corner.
pub type ShapeBox = [u64; 6];

pub struct Extract {
    /// The names of the fluids and of the block entity types by the game's ids.
    pub fluids: Vec<String>,
    pub block_entity_types: Vec<String>,
    /// One for each block state, by state id.
    pub states: Vec<StateDump>,
    /// One for each block, by block id.
    pub blocks: Vec<BlockDump>,
    /// The biome parameter lists the game knows, sorted by name.
    pub biome_parameters: Vec<ParameterList>,
    /// What the game's noise routers give at fixed positions, by the name of the file
    /// (`overworld_13579`: the noise settings and the seed), sorted.
    pub router_values: Vec<(String, RouterValues)>,
}

/// What the game says of one block state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateDump {
    pub block: String,
    pub light_emission: u8,
    pub light_dampening: u8,
    /// The twelve flags the program asks the state for, in the order of the committed
    /// row (ADR-0019, section 5); the two bits of the heightmap tags are not among
    /// them.
    pub flags: u16,
    /// The sturdy faces for the support types full, centre and rigid, a bit a face.
    pub sturdy: [u8; 3],
    /// The name of the game's `PushReaction`.
    pub push_reaction: String,
    pub fluid: Option<FluidDump>,
    /// The offset of the position the state marks for post-processing.
    pub post_process: Option<[i32; 3]>,
    /// The occlusion shape of each face, in the order of the game's `Direction`.
    pub occlusion_faces: [Vec<ShapeBox>; 6],
    pub collision: Vec<ShapeBox>,
    /// The questions whose answer looked at the level or changed with the position,
    /// such as `collision:block_entity`. Empty for a state that answers by itself.
    pub consulted: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluidDump {
    /// The fluid's id in the game's registry.
    pub id: usize,
    pub amount: u8,
    pub source: bool,
    pub falling: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDump {
    pub name: String,
    /// The simple name of the block's Java class.
    pub class: String,
    pub block_entity_type: Option<usize>,
    pub used_by_click: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterList {
    /// The path of the preset's name: `overworld` or `nether`.
    pub name: String,
    pub entries: Vec<ParameterEntry>,
}

/// One entry of a parameter list, in the integers the game holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterEntry {
    /// The lower and upper bound of temperature, humidity, continentalness, erosion,
    /// depth and weirdness.
    pub bounds: [[i64; 2]; 6],
    pub offset: i64,
    pub biome: String,
}

impl Extract {
    pub fn load(directory: &Path) -> Result<Self> {
        let read = |name: &str| {
            let path = directory.join(name);
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
        };

        let mut biome_parameters = Vec::new();
        let mut names = Vec::new();
        let mut router_names = Vec::new();
        for entry in
            fs::read_dir(directory).with_context(|| format!("listing {}", directory.display()))?
        {
            let file_name = entry?.file_name();
            let file_name = file_name.to_str().context("a file name is not UTF-8")?;
            if let Some(name) = file_name
                .strip_prefix("biome_parameters_")
                .and_then(|rest| rest.strip_suffix(".txt"))
            {
                names.push(name.to_owned());
            }
            if let Some(name) = file_name
                .strip_prefix("router_")
                .and_then(|rest| rest.strip_suffix(".txt"))
            {
                router_names.push(name.to_owned());
            }
        }
        names.sort();
        router_names.sort();
        let mut router_values = Vec::new();
        for name in router_names {
            let values = RouterValues::parse(&read(&format!("router_{name}.txt"))?)
                .with_context(|| format!("reading the router's values {name}"))?;
            router_values.push((name, values));
        }
        for name in names {
            let text = read(&format!("biome_parameters_{name}.txt"))?;
            biome_parameters.push(ParameterList {
                entries: parameter_entries(&text)
                    .with_context(|| format!("reading the parameter list {name}"))?,
                name,
            });
        }

        Ok(Self {
            fluids: names_by_id(&read("fluids.txt")?).context("reading fluids.txt")?,
            block_entity_types: names_by_id(&read("block_entity_types.txt")?)
                .context("reading block_entity_types.txt")?,
            states: states(&read("block_states.txt")?).context("reading block_states.txt")?,
            blocks: blocks(&read("blocks.txt")?).context("reading blocks.txt")?,
            biome_parameters,
            router_values,
        })
    }
}

/// Lines of an id and a name; the ids have to count up from zero.
fn names_by_id(text: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [id, name] = fields[..] else {
            bail!("a line has {} fields where two are expected", fields.len());
        };
        ensure!(
            integer::<usize>(id)? == names.len(),
            "the ids have a gap at {id}"
        );
        names.push(name.to_owned());
    }
    Ok(names)
}

/// The lines of `block_states.txt`.
pub fn states(text: &str) -> Result<Vec<StateDump>> {
    let mut states = Vec::new();
    for line in text.lines() {
        let state = state(line, states.len())
            .with_context(|| format!("in the line of state {}", states.len()))?;
        states.push(state);
    }
    Ok(states)
}

fn state(line: &str, expected_id: usize) -> Result<StateDump> {
    let fields: Vec<&str> = line.split('\t').collect();
    ensure!(
        fields.len() == 22,
        "{} fields where 22 are expected",
        fields.len()
    );
    ensure!(
        integer::<usize>(fields[0])? == expected_id,
        "the state ids have a gap"
    );
    let flags: u16 = integer(fields[4])?;
    ensure!(flags < 1 << 12, "more flags than the twelve the row has");

    let fluid_id: i64 = integer(fields[9])?;
    let fluid = if fluid_id < 0 {
        None
    } else {
        Some(FluidDump {
            id: usize::try_from(fluid_id)?,
            amount: integer(fields[10])?,
            source: boolean(fields[11])?,
            falling: boolean(fields[12])?,
        })
    };

    let post_process = if fields[13] == "none" {
        None
    } else {
        let parts: Vec<&str> = fields[13].split(',').collect();
        let [x, y, z] = parts[..] else {
            bail!("{} is not an offset", fields[13]);
        };
        Some([integer(x)?, integer(y)?, integer(z)?])
    };

    let mut occlusion_faces: [Vec<ShapeBox>; 6] = Default::default();
    for (face, text) in occlusion_faces.iter_mut().zip(&fields[14..20]) {
        *face = shape(text)?;
    }

    Ok(StateDump {
        block: fields[1].to_owned(),
        light_emission: integer(fields[2])?,
        light_dampening: integer(fields[3])?,
        flags,
        sturdy: [
            integer(fields[5])?,
            integer(fields[6])?,
            integer(fields[7])?,
        ],
        push_reaction: fields[8].to_owned(),
        fluid,
        post_process,
        occlusion_faces,
        collision: shape(fields[20])?,
        consulted: match fields[21] {
            "-" => Vec::new(),
            listed => listed.split(',').map(str::to_owned).collect(),
        },
    })
}

/// A shape: `-` for the empty one, otherwise boxes separated by `;`, each six doubles
/// as their bits in hexadecimal, separated by commas.
fn shape(text: &str) -> Result<Vec<ShapeBox>> {
    if text == "-" {
        return Ok(Vec::new());
    }
    let mut boxes = Vec::new();
    for box_text in text.split(';') {
        let numbers = box_text
            .split(',')
            .map(|bits| u64::from_str_radix(bits, 16))
            .collect::<Result<Vec<u64>, _>>()
            .with_context(|| format!("{box_text} is not a box"))?;
        let shape_box: ShapeBox = numbers
            .try_into()
            .map_err(|_| anyhow::anyhow!("{box_text} does not have six numbers"))?;
        boxes.push(shape_box);
    }
    Ok(boxes)
}

/// The lines of `blocks.txt`.
pub fn blocks(text: &str) -> Result<Vec<BlockDump>> {
    let mut blocks = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        // The last two fields say how "used by a click" came about, for a reader.
        ensure!(
            fields.len() == 7,
            "a line has {} fields where 7 are expected",
            fields.len()
        );
        ensure!(
            integer::<usize>(fields[0])? == blocks.len(),
            "the block ids have a gap at {}",
            fields[0]
        );
        let entity_type: i64 = integer(fields[3])?;
        blocks.push(BlockDump {
            name: fields[1].to_owned(),
            class: fields[2].to_owned(),
            block_entity_type: if entity_type < 0 {
                None
            } else {
                Some(usize::try_from(entity_type)?)
            },
            used_by_click: boolean(fields[4])?,
        });
    }
    Ok(blocks)
}

/// The lines of one `biome_parameters_*.txt`.
pub fn parameter_entries(text: &str) -> Result<Vec<ParameterEntry>> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        ensure!(
            fields.len() == 14,
            "a line has {} fields where 14 are expected",
            fields.len()
        );
        let mut bounds = [[0; 2]; 6];
        for (parameter, slot) in bounds.iter_mut().enumerate() {
            *slot = [
                integer(fields[2 * parameter])?,
                integer(fields[2 * parameter + 1])?,
            ];
        }
        entries.push(ParameterEntry {
            bounds,
            offset: integer(fields[12])?,
            biome: fields[13].to_owned(),
        });
    }
    Ok(entries)
}

fn integer<T: std::str::FromStr>(text: &str) -> Result<T> {
    text.parse()
        .map_err(|_| anyhow::anyhow!("{text:?} is not a number that fits"))
}

fn boolean(text: &str) -> Result<bool> {
    match text {
        "0" => Ok(false),
        "1" => Ok(true),
        other => bail!("{other:?} is neither 0 nor 1"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "3ff0000000000000";
    const HALF: &str = "3fe0000000000000";

    #[test]
    fn a_line_of_a_state_is_read_field_by_field() {
        let full = format!("0,0,0,{ONE},{ONE},{ONE}");
        let lower = format!("0,0,0,{ONE},{HALF},{ONE}");
        let line = format!(
            "0\tminecraft:thing\t7\t1\t2086\t63\t2\t1\tPOPPED\t2\t8\t1\t0\t0,1,0\t\
             -\t{full}\t{lower}\t{lower}\t{lower}\t{lower}\t{lower};{full}\tcollision:block_entity"
        );
        let read = states(&line).unwrap();
        assert_eq!(read.len(), 1);
        let state = &read[0];
        assert_eq!(state.block, "minecraft:thing");
        assert_eq!((state.light_emission, state.light_dampening), (7, 1));
        assert_eq!(state.flags, 2086);
        assert_eq!(state.sturdy, [63, 2, 1]);
        assert_eq!(state.push_reaction, "POPPED");
        assert_eq!(
            state.fluid,
            Some(FluidDump {
                id: 2,
                amount: 8,
                source: true,
                falling: false
            })
        );
        assert_eq!(state.post_process, Some([0, 1, 0]));
        assert!(state.occlusion_faces[0].is_empty());
        let one = 1.0f64.to_bits();
        assert_eq!(state.occlusion_faces[1], [[0, 0, 0, one, one, one]]);
        assert_eq!(state.occlusion_faces[2][0][4], 0.5f64.to_bits());
        assert_eq!(state.collision.len(), 2);
        assert_eq!(state.consulted, ["collision:block_entity"]);
    }

    #[test]
    fn a_state_out_of_order_or_with_a_field_missing_is_refused() {
        let line =
            "1\tminecraft:thing\t0\t0\t0\t0\t0\t0\tPUSH\t-1\t0\t0\t0\tnone\t-\t-\t-\t-\t-\t-\t-\t-";
        assert!(states(line).is_err(), "the first id has to be 0");
        let line =
            "0\tminecraft:thing\t0\t0\t0\t0\t0\t0\tPUSH\t-1\t0\t0\t0\tnone\t-\t-\t-\t-\t-\t-\t-";
        assert!(states(line).is_err(), "a field is missing");
    }

    #[test]
    fn lines_of_blocks_and_of_parameters_are_read() {
        let read = blocks(
            "0\tminecraft:air\tAirBlock\t-1\t0\t-\t-\n\
             1\tminecraft:chest\tChestBlock\t1\t1\tChestBlock\t-\n",
        )
        .unwrap();
        assert_eq!(read[1].class, "ChestBlock");
        assert_eq!(read[1].block_entity_type, Some(1));
        assert!(read[1].used_by_click && !read[0].used_by_click);
        assert_eq!(read[0].block_entity_type, None);

        let read = parameter_entries(
            "-10000\t10000\t1\t2\t3\t4\t5\t6\t7\t8\t9\t10\t3750\tminecraft:plains\n",
        )
        .unwrap();
        assert_eq!(read[0].bounds[0], [-10000, 10000]);
        assert_eq!(read[0].bounds[5], [9, 10]);
        assert_eq!(read[0].offset, 3750);
        assert_eq!(read[0].biome, "minecraft:plains");
    }
}
