//! Packs the two tables that are thousands of rows of one shape: what the game says of
//! every block state, and the biome parameter lists (ADR-0019, sections 1 and 5).
//!
//! Both are functions of what the extract program and the data generator wrote and of
//! nothing else, so the same inputs give the same bytes.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};

use crate::extract::{ParameterList, ShapeBox, StateDump};
use crate::model::{BiomeReport, Block};
use crate::packed::{self, Section};

/// The layout of `block_states.bin`; the reader in `clustine-data` names the same.
pub const BLOCK_STATES_LAYOUT: u16 = 1;
/// The layout of `biome_parameters.bin`.
pub const BIOME_PARAMETERS_LAYOUT: u16 = 1;

/// The parameter lists that are packed, each a section, in this order.
pub const PARAMETER_LISTS: [&str; 2] = ["overworld", "nether"];

/// The game's `PushReaction` by the number a row holds.
pub const PUSH_REACTIONS: [&str; 5] =
    ["PUSH_PULL", "PUSH", "POPPED", "IMMOVEABLE", "IGNORE_ENTITY"];

/// The names of the flags of a row, bit 0 first.
pub const FLAGS: [&str; 15] = [
    "air",
    "solid",
    "solid_render",
    "liquid",
    "replaceable",
    "occludes",
    "occludes_by_shape",
    "skylight_down",
    "light_permeable",
    "block_entity",
    "collision_empty",
    "collision_full",
    "motion_heightmap",
    "motion_heightmap_no_leaves",
    "beyond_its_state",
];

const FLAG_MOTION_HEIGHTMAP: u16 = 1 << 12;
const FLAG_MOTION_HEIGHTMAP_NO_LEAVES: u16 = 1 << 13;
const FLAG_BEYOND_ITS_STATE: u16 = 1 << 14;
const FLAG_OCCLUDES_BY_SHAPE: u16 = 1 << 6;

/// A block whose answers are not values of its state alone, and what is done about it.
pub struct BeyondItsState {
    pub block: &'static str,
    /// What the answers may have looked at, as the extract program names it.
    pub consulted: &'static [&'static str],
    /// What the committed row holds for it; the reader in `clustine-data` says the
    /// same where it documents the flag.
    #[allow(dead_code, reason = "it is the record of the decision, read by people")]
    pub done: &'static str,
}

const BLOCK_ENTITY: &[&str] = &["collision:block_entity", "sturdy:block_entity"];
const SHULKER_BOX: &str = "Its shape is the box's as its block entity has it open. The row \
     holds the answer without a block entity: the closed box, a full block. The flag says so.";
const SHIFTED: &str = "Its shape is shifted sideways by a number made from its position. The \
     row holds the shape at (0, 0, 0); whether it is empty or full does not change. The flag \
     says so.";

/// The blocks that may look at the level or answer by position. A state of any other
/// block that does so fails the run, so that the day a jar adds one is noticed.
pub const BEYOND_ITS_STATE: &[BeyondItsState] = &[
    BeyondItsState {
        block: "minecraft:moving_piston",
        consulted: BLOCK_ENTITY,
        done: "Its shape is the moved block's, from its block entity. The row holds the \
               answer without one: nothing to collide with. The flag says so.",
    },
    BeyondItsState {
        block: "minecraft:shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:white_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:orange_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:magenta_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:light_blue_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:yellow_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:lime_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:pink_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:gray_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:light_gray_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:cyan_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:purple_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:blue_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:brown_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:green_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:red_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:black_shulker_box",
        consulted: BLOCK_ENTITY,
        done: SHULKER_BOX,
    },
    BeyondItsState {
        block: "minecraft:bamboo",
        consulted: &["collision:position"],
        done: SHIFTED,
    },
    BeyondItsState {
        block: "minecraft:pointed_dripstone",
        consulted: &["collision:position", "sturdy:position"],
        done: SHIFTED,
    },
    BeyondItsState {
        block: "minecraft:sulfur_spike",
        consulted: &["collision:position", "sturdy:position"],
        done: SHIFTED,
    },
];

/// Distinct values in the order they first came, each with its number.
struct Distinct<T: Ord + Clone> {
    numbers: BTreeMap<T, usize>,
    values: Vec<T>,
}

impl<T: Ord + Clone> Distinct<T> {
    /// Starts with values whose numbers the layout fixes.
    fn starting_with(fixed: impl IntoIterator<Item = T>) -> Self {
        let mut distinct = Self {
            numbers: BTreeMap::new(),
            values: Vec::new(),
        };
        for value in fixed {
            distinct.number(value);
        }
        distinct
    }

    fn number(&mut self, value: T) -> usize {
        if let Some(&number) = self.numbers.get(&value) {
            return number;
        }
        let number = self.values.len();
        self.numbers.insert(value.clone(), number);
        self.values.push(value);
        number
    }
}

/// A rectangle on a face as the bits of four doubles: min u, min v, max u, max v.
type Rectangle = [u64; 4];

const ZERO: u64 = 0;
const ONE: u64 = 0x3ff0_0000_0000_0000;

/// What a face shape, which the game gives as boxes, covers of the face `face` (in the
/// order of the game's `Direction`): each box without the axis the face looks along.
///
/// The game's face shape is a slice that reaches from one side of the block to the
/// other along that axis; one that does not is something this table has no place for.
fn face_rectangles(boxes: &[ShapeBox], face: usize) -> Result<Vec<Rectangle>> {
    // Down and up look along y, north and south along z, west and east along x.
    let axis = [1, 1, 2, 2, 0, 0][face];
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    boxes
        .iter()
        .map(|shape_box| {
            ensure!(
                shape_box[axis] == ZERO && shape_box[axis + 3] == ONE,
                "a face shape does not reach through the block"
            );
            Ok([
                shape_box[u],
                shape_box[v],
                shape_box[u + 3],
                shape_box[v + 3],
            ])
        })
        .collect()
}

/// `block_states.bin`: one row of 16 bytes for each state, and the shapes behind them.
///
/// `blocks` are the blocks of the report with their ranges of states; `in_heightmap`
/// and `in_heightmap_no_leaves` are the ids of the blocks in the two heightmap tags;
/// `fluid_count` is the size of the fluid registry.
pub fn block_states(
    states: &[StateDump],
    blocks: &[Block],
    in_heightmap: &BTreeSet<usize>,
    in_heightmap_no_leaves: &BTreeSet<usize>,
    fluid_count: usize,
    sha1: &str,
) -> Result<Vec<u8>> {
    let state_count = blocks.last().map_or(0, |block| block.last_state + 1);
    ensure!(
        states.len() as u64 == state_count,
        "the extract program wrote {} block states, the report has {state_count}",
        states.len()
    );

    let mut rows = Section::new(16);
    let mut face_shapes: Distinct<Vec<Rectangle>> =
        Distinct::starting_with([Vec::new(), vec![[ZERO, ZERO, ONE, ONE]]]);
    let mut face_sets: Distinct<[u16; 6]> = Distinct::starting_with([[0; 6], [1; 6]]);
    let mut collision_shapes: Distinct<Vec<ShapeBox>> =
        Distinct::starting_with([Vec::new(), vec![[ZERO, ZERO, ZERO, ONE, ONE, ONE]]]);
    let mut post_offsets: Distinct<[i8; 3]> = Distinct::starting_with([]);

    for (block_id, block) in blocks.iter().enumerate() {
        for id in block.first_state..=block.last_state {
            let state = &states[id as usize];
            let row = state_row(
                state,
                &block.name,
                in_heightmap.contains(&block_id),
                in_heightmap_no_leaves.contains(&block_id),
                fluid_count,
                &mut face_shapes,
                &mut face_sets,
                &mut collision_shapes,
                &mut post_offsets,
            )
            .with_context(|| format!("block state {id} ({})", block.name))?;
            rows.push(&row);
        }
    }

    let mut sets = Section::new(12);
    for set in &face_sets.values {
        let bytes: Vec<u8> = set.iter().flat_map(|shape| shape.to_le_bytes()).collect();
        sets.push(&bytes);
    }
    let (face_starts, rectangles) = flattened(&face_shapes.values, 32)?;
    let (collision_starts, boxes) = flattened(&collision_shapes.values, 48)?;
    let mut offsets = Section::new(3);
    for offset in &post_offsets.values {
        offsets.push(&offset.map(|part| part as u8));
    }

    packed::file(
        packed::KIND_BLOCK_STATES,
        BLOCK_STATES_LAYOUT,
        sha1,
        &[
            rows,
            sets,
            face_starts,
            rectangles,
            collision_starts,
            boxes,
            offsets,
        ],
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "the four tables of distinct values are filled as the rows are made"
)]
fn state_row(
    state: &StateDump,
    block: &str,
    in_heightmap: bool,
    in_heightmap_no_leaves: bool,
    fluid_count: usize,
    face_shapes: &mut Distinct<Vec<Rectangle>>,
    face_sets: &mut Distinct<[u16; 6]>,
    collision_shapes: &mut Distinct<Vec<ShapeBox>>,
    post_offsets: &mut Distinct<[i8; 3]>,
) -> Result<[u8; 16]> {
    ensure!(
        state.block == block,
        "the extract program has {} where the report has this block",
        state.block
    );

    let mut flags = state.flags;
    ensure!(
        flags < 1 << 12,
        "more flags than the twelve the program asks for"
    );
    if in_heightmap {
        flags |= FLAG_MOTION_HEIGHTMAP;
    }
    if in_heightmap_no_leaves {
        flags |= FLAG_MOTION_HEIGHTMAP_NO_LEAVES;
    }
    if !state.consulted.is_empty() {
        let listed = BEYOND_ITS_STATE
            .iter()
            .find(|listed| listed.block == block)
            .with_context(|| {
                format!(
                    "its answer is not a value of the state alone ({}) and the block is not \
                     listed in BEYOND_ITS_STATE with what is done about that",
                    state.consulted.join(", ")
                )
            })?;
        for consulted in &state.consulted {
            ensure!(
                listed.consulted.contains(&consulted.as_str()),
                "its answer looked at {consulted}, which BEYOND_ITS_STATE does not expect of it"
            );
        }
        flags |= FLAG_BEYOND_ITS_STATE;
    }

    let mut row = [0; 16];
    row[0] = state.light_emission;
    row[1] = state.light_dampening;
    ensure!(
        row[0] <= 15 && row[1] <= 15,
        "light above 15: emission {}, dampening {}",
        row[0],
        row[1]
    );
    row[2..4].copy_from_slice(&flags.to_le_bytes());
    for (slot, faces) in row[4..7].iter_mut().zip(state.sturdy) {
        ensure!(faces < 1 << 6, "more sturdy faces than a block has");
        *slot = faces;
    }
    row[7] = PUSH_REACTIONS
        .iter()
        .position(|known| *known == state.push_reaction)
        .with_context(|| format!("the push reaction {} is not known", state.push_reaction))?
        as u8;

    if let Some(fluid) = state.fluid {
        ensure!(
            fluid.id < fluid_count && fluid.id < 255,
            "fluid {} is not in the registry",
            fluid.id
        );
        ensure!(
            (1..64).contains(&fluid.amount),
            "a fluid's amount of {} does not fit its six bits",
            fluid.amount
        );
        row[8] = fluid.id as u8 + 1;
        row[9] = fluid.amount
            | if fluid.source { 0x40 } else { 0 }
            | if fluid.falling { 0x80 } else { 0 };
    }

    row[10] = match state.post_process {
        None => 0,
        Some([0, 0, 0]) => 1,
        Some(offset) => {
            let mut small = [0i8; 3];
            for (slot, part) in small.iter_mut().zip(offset) {
                *slot = i8::try_from(part)
                    .ok()
                    .context("a post-processing offset does not fit a byte")?;
            }
            u8::try_from(post_offsets.number(small) + 2)
                .ok()
                .context("more post-processing offsets than a byte can name")?
        }
    };

    // Light looks at the faces only of a state that occludes by its shape; every other
    // state covers nothing as far as shapes go, and stops light by its dampening.
    let set = if flags & FLAG_OCCLUDES_BY_SHAPE != 0 {
        let mut set = [0u16; 6];
        for (face, slot) in set.iter_mut().enumerate() {
            let rectangles = face_rectangles(&state.occlusion_faces[face], face)?;
            *slot = u16::try_from(face_shapes.number(rectangles))?;
        }
        face_sets.number(set)
    } else {
        0
    };
    row[12..14].copy_from_slice(&u16::try_from(set)?.to_le_bytes());

    let collision = collision_shapes.number(state.collision.clone());
    // The two flags and the shape are asked separately; they have to agree.
    ensure!(
        (flags & (1 << 10) != 0) == (collision == 0),
        "the flag for an empty collision shape contradicts the shape"
    );
    ensure!(
        flags & (1 << 11) == 0 || collision == 1,
        "the flag for a full collision shape is set and the shape is not the full block"
    );
    row[14..16].copy_from_slice(&u16::try_from(collision)?.to_le_bytes());
    Ok(row)
}

/// Lists of rows as two sections: where each list starts in the second, with one more
/// entry for the end, and the rows of all lists one after the other.
fn flattened<const N: usize>(
    lists: &[Vec<[u64; N]>],
    row_bytes: usize,
) -> Result<(Section, Section)> {
    let mut starts = Section::new(4);
    let mut rows = Section::new(row_bytes);
    for list in lists {
        starts.push(&u32::try_from(rows.rows())?.to_le_bytes());
        for row in list {
            let bytes: Vec<u8> = row.iter().flat_map(|bits| bits.to_le_bytes()).collect();
            rows.push(&bytes);
        }
    }
    starts.push(&u32::try_from(rows.rows())?.to_le_bytes());
    Ok((starts, rows))
}

/// `biome_parameters.bin`: one section for each list of [`PARAMETER_LISTS`], rows in the
/// list's own order: twelve `i16` for the bounds, one for the offset, and the biome's
/// id in `biomes`, which is Clustine's sorted registry.
///
/// Every number is checked forwards against the data generator's report, which has
/// them as decimals: the game's integer, divided by 10,000 in single precision, has to
/// be the report's number in single precision. (Going backwards, from the report's
/// number to an integer, would fail on right values.)
pub fn biome_parameters(
    lists: &[ParameterList],
    reports: &[BiomeReport],
    biomes: &[String],
    sha1: &str,
) -> Result<Vec<u8>> {
    let names: Vec<&str> = lists.iter().map(|list| list.name.as_str()).collect();
    let mut expected = PARAMETER_LISTS;
    expected.sort_unstable();
    ensure!(
        names == expected,
        "the game knows the parameter lists {names:?}, this table is laid out for {expected:?}"
    );

    let mut sections = Vec::new();
    for name in PARAMETER_LISTS {
        let list = lists
            .iter()
            .find(|list| list.name == name)
            .context("a parameter list went missing")?;
        let report = reports
            .iter()
            .find(|report| report.name == name)
            .with_context(|| format!("the data generator has no report of the list {name}"))?;
        ensure!(
            list.entries.len() == report.entries.len(),
            "the list {name} has {} entries, its report {}",
            list.entries.len(),
            report.entries.len()
        );

        let mut section = Section::new(27);
        for (index, (entry, reported)) in list.entries.iter().zip(&report.entries).enumerate() {
            let what = || format!("entry {index} of the list {name} ({})", entry.biome);
            ensure!(
                entry.biome == reported.biome,
                "{} is {} in the report",
                what(),
                reported.biome
            );
            let biome = biomes
                .iter()
                .position(|known| *known == entry.biome)
                .with_context(|| format!("{}: the biome is not in the registry", what()))?;
            let biome = u8::try_from(biome)
                .ok()
                .with_context(|| format!("{}: the biome's id does not fit a byte", what()))?;

            let mut row = Vec::with_capacity(27);
            let numbers = entry
                .bounds
                .iter()
                .flatten()
                .copied()
                .zip(reported.bounds.iter().flatten().copied())
                .chain([(entry.offset, reported.offset)]);
            for (number, reported) in numbers {
                let small = i16::try_from(number).ok().with_context(|| {
                    format!("{}: the parameter {number} does not fit 16 bits", what())
                })?;
                if !confirms(number, reported) {
                    bail!(
                        "{}: the game holds {number}, which the report's {reported} does not \
                         confirm",
                        what()
                    );
                }
                row.extend_from_slice(&small.to_le_bytes());
            }
            row.push(biome);
            section.push(&row);
        }
        sections.push(section);
    }
    packed::file(
        packed::KIND_BIOME_PARAMETERS,
        BIOME_PARAMETERS_LAYOUT,
        sha1,
        &sections,
    )
}

/// Whether the report's decimal is what the game writes for its integer `number`: the
/// integer divided by 10,000 in single precision.
///
/// The report's number arrives here as a double. It was printed from a `float` with no
/// more digits than tell it from its neighbours, and such a decimal read as a double
/// and narrowed is that `float` again.
fn confirms(number: i64, reported: f64) -> bool {
    number as f32 / 10000.0 == reported as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{FluidDump, ParameterEntry};
    use crate::model::BiomeReportEntry;

    const SHA1: &str = "33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c";
    const HALF: u64 = 0x3fe0_0000_0000_0000;
    const FULL_BOX: ShapeBox = [ZERO, ZERO, ZERO, ONE, ONE, ONE];

    fn block(name: &str, first: u64, last: u64) -> Block {
        Block {
            name: name.to_owned(),
            first_state: first,
            last_state: last,
            default_state: first,
            properties: Vec::new(),
        }
    }

    fn state(block: &str) -> StateDump {
        StateDump {
            block: block.to_owned(),
            light_emission: 0,
            light_dampening: 0,
            // Nothing collides with it.
            flags: 1 << 10,
            sturdy: [0; 3],
            push_reaction: "PUSH_PULL".to_owned(),
            fluid: None,
            post_process: None,
            occlusion_faces: Default::default(),
            collision: Vec::new(),
            consulted: Vec::new(),
        }
    }

    fn solid(block: &str) -> StateDump {
        StateDump {
            light_dampening: 15,
            flags: 1 << 1 | 1 << 2 | 1 << 5 | 1 << 11,
            sturdy: [63; 3],
            collision: vec![FULL_BOX],
            occlusion_faces: std::array::from_fn(|_| vec![FULL_BOX]),
            ..state(block)
        }
    }

    /// A lower slab: it occludes by its shape, covers the bottom and half of each side.
    fn slab(block: &str) -> StateDump {
        let lower: ShapeBox = [ZERO, ZERO, ZERO, ONE, HALF, ONE];
        let mut faces: [Vec<ShapeBox>; 6] = Default::default();
        faces[0] = vec![FULL_BOX];
        // North and south look along z, west and east along x; the slice reaches
        // through the block along that axis.
        for face in &mut faces[2..6] {
            *face = vec![[ZERO, ZERO, ZERO, ONE, HALF, ONE]];
        }
        StateDump {
            light_dampening: 1,
            flags: 1 << 1 | 1 << 5 | 1 << 6,
            sturdy: [1, 1, 1],
            collision: vec![lower],
            occlusion_faces: faces,
            ..state(block)
        }
    }

    fn pack(states: &[StateDump], blocks: &[Block]) -> Result<Vec<u8>> {
        block_states(
            states,
            blocks,
            &BTreeSet::from([1]),
            &BTreeSet::from([1]),
            5,
            SHA1,
        )
    }

    fn small_world() -> (Vec<StateDump>, Vec<Block>) {
        let mut water = state("minecraft:water");
        water.fluid = Some(FluidDump {
            id: 2,
            amount: 8,
            source: true,
            falling: false,
        });
        let mut falling = state("minecraft:water");
        falling.fluid = Some(FluidDump {
            id: 1,
            amount: 8,
            source: false,
            falling: true,
        });
        let mut magma = solid("minecraft:magma_block");
        magma.light_emission = 3;
        magma.post_process = Some([0, 1, 0]);
        let states = vec![
            state("minecraft:air"),
            solid("minecraft:stone"),
            water,
            falling,
            slab("minecraft:slab"),
            magma,
        ];
        let blocks = vec![
            block("minecraft:air", 0, 0),
            block("minecraft:stone", 1, 1),
            block("minecraft:water", 2, 3),
            block("minecraft:slab", 4, 4),
            block("minecraft:magma_block", 5, 5),
        ];
        (states, blocks)
    }

    #[test]
    fn the_rows_of_a_small_table_hold_what_the_layout_says() {
        let (states, blocks) = small_world();
        let bytes = pack(&states, &blocks).unwrap();
        let table = packed::parse(&bytes).unwrap();
        assert_eq!(table.kind, packed::KIND_BLOCK_STATES);
        assert_eq!(table.sha1, packed::sha1_bytes(SHA1).unwrap());
        let row_bytes: Vec<usize> = table.sections.iter().map(|s| s.row_bytes).collect();
        assert_eq!(row_bytes, [16, 12, 4, 32, 4, 48, 3]);
        let rows: Vec<&[u8]> = table.sections[0].rows().collect();
        assert_eq!(rows.len(), 6);

        // Air: nothing set but the empty collision shape, sets and shapes 0.
        assert_eq!(rows[0], [0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        // Stone is block 1, which is in both tags; it does not occlude by shape, so
        // its faces are set 0, and its collision shape is the full block, shape 1.
        let stone_flags = u16::from_le_bytes([rows[1][2], rows[1][3]]);
        assert_eq!(
            stone_flags,
            1 << 1 | 1 << 2 | 1 << 5 | 1 << 11 | 1 << 12 | 1 << 13
        );
        assert_eq!(&rows[1][4..7], [63, 63, 63]);
        assert_eq!(&rows[1][12..16], [0, 0, 1, 0]);
        // A water source: fluid 2 plus one, eight with the source bit.
        assert_eq!(&rows[2][8..10], [3, 8 | 0x40]);
        // Falling water: fluid 1 plus one, eight with the falling bit.
        assert_eq!(&rows[3][8..10], [2, 8 | 0x80]);
        // The slab has the first set and the first collision shape that are not fixed.
        assert_eq!(&rows[4][12..16], [2, 0, 2, 0]);
        let set: Vec<u16> = table.sections[1]
            .row(2)
            .unwrap()
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        // Bottom full, top empty. The lower half is one shape on north and south, which
        // lie in x and y, and another on west and east, which lie in y and z.
        assert_eq!(set, [1, 0, 2, 2, 3, 3]);
        // Magma marks the block above: the first offset, so 2.
        assert_eq!(rows[5][10], 2);
        assert_eq!(table.sections[6].row(0).unwrap(), [0, 1, 0]);

        // The empty shape has no rectangle, the full one and the two halves one each.
        let starts: Vec<u32> = table.sections[2]
            .rows()
            .map(|row| u32::from_le_bytes([row[0], row[1], row[2], row[3]]))
            .collect();
        assert_eq!(starts, [0, 0, 1, 2, 3]);
        let rectangle = |row: usize| -> Vec<f64> {
            table.sections[3]
                .row(row)
                .unwrap()
                .chunks(8)
                .map(|bits| f64::from_le_bytes(bits.try_into().unwrap()))
                .collect()
        };
        assert_eq!(rectangle(0), [0.0, 0.0, 1.0, 1.0]);
        // North: x from 0 to 1, y from 0 to ½. West: y from 0 to ½, z from 0 to 1.
        assert_eq!(rectangle(1), [0.0, 0.0, 1.0, 0.5]);
        assert_eq!(rectangle(2), [0.0, 0.0, 0.5, 1.0]);
    }

    #[test]
    fn packing_the_same_states_twice_gives_the_same_bytes() {
        let (states, blocks) = small_world();
        assert_eq!(
            pack(&states, &blocks).unwrap(),
            pack(&states, &blocks).unwrap()
        );
    }

    #[test]
    fn a_state_that_consulted_the_level_fails_unless_its_block_is_listed() {
        let (mut states, blocks) = small_world();
        states[1].consulted = vec!["collision:block_state".to_owned()];
        let error = pack(&states, &blocks).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("minecraft:stone"), "{message}");
        assert!(message.contains("BEYOND_ITS_STATE"), "{message}");
    }

    #[test]
    fn a_listed_block_gets_the_flag_and_fails_on_what_is_not_expected_of_it() {
        let mut bamboo = state("minecraft:bamboo");
        bamboo.consulted = vec!["collision:position".to_owned()];
        let blocks = [block("minecraft:bamboo", 0, 0)];
        let bytes = pack(&[bamboo.clone()], &blocks).unwrap();
        let table = packed::parse(&bytes).unwrap();
        let row = table.sections[0].row(0).unwrap();
        assert_ne!(
            u16::from_le_bytes([row[2], row[3]]) & FLAG_BEYOND_ITS_STATE,
            0
        );

        bamboo.consulted = vec!["collision:block_state".to_owned()];
        assert!(pack(&[bamboo], &blocks).is_err());
    }

    #[test]
    fn states_that_do_not_match_the_report_fail() {
        let (states, blocks) = small_world();
        assert!(pack(&states[..5], &blocks).is_err(), "a state is missing");
        let (mut states, blocks) = small_world();
        states[1].block = "minecraft:granite".to_owned();
        assert!(pack(&states, &blocks).is_err(), "another block");
        let (mut states, blocks) = small_world();
        states[1].push_reaction = "SHATTERED".to_owned();
        assert!(pack(&states, &blocks).is_err(), "an unknown push reaction");
        let (mut states, blocks) = small_world();
        states[1].collision.clear();
        assert!(
            pack(&states, &blocks).is_err(),
            "full by its flag, empty by its shape"
        );
    }

    fn entry(bounds: [[i64; 2]; 6], offset: i64, biome: &str) -> ParameterEntry {
        ParameterEntry {
            bounds,
            offset,
            biome: biome.to_owned(),
        }
    }

    /// The report's entry for the game's entry, as the game would print it.
    fn reported(entry: &ParameterEntry) -> BiomeReportEntry {
        let decimal = |number: i64| f64::from(number as f32 / 10000.0);
        BiomeReportEntry {
            biome: entry.biome.clone(),
            bounds: entry.bounds.map(|pair| pair.map(decimal)),
            offset: decimal(entry.offset),
        }
    }

    fn small_lists() -> (Vec<ParameterList>, Vec<BiomeReport>, Vec<String>) {
        let overworld = vec![
            entry(
                [
                    [-10000, 10000],
                    [-3500, -1000],
                    [-12000, -10500],
                    [0, 0],
                    [10000, 10000],
                    [1, 2],
                ],
                0,
                "minecraft:plains",
            ),
            entry([[4000, 4000]; 6], 3750, "minecraft:desert"),
        ];
        let nether = vec![entry([[0, 0]; 6], 1750, "minecraft:basalt_deltas")];
        let reports = vec![
            BiomeReport {
                name: "overworld".to_owned(),
                entries: overworld.iter().map(reported).collect(),
            },
            BiomeReport {
                name: "nether".to_owned(),
                entries: nether.iter().map(reported).collect(),
            },
        ];
        let lists = vec![
            ParameterList {
                name: "nether".to_owned(),
                entries: nether,
            },
            ParameterList {
                name: "overworld".to_owned(),
                entries: overworld,
            },
        ];
        let biomes = ["basalt_deltas", "desert", "plains"]
            .map(|name| format!("minecraft:{name}"))
            .to_vec();
        (lists, reports, biomes)
    }

    #[test]
    fn parameter_lists_are_packed_in_their_own_order_with_sorted_biome_ids() {
        let (lists, reports, biomes) = small_lists();
        let bytes = biome_parameters(&lists, &reports, &biomes, SHA1).unwrap();
        assert_eq!(
            bytes,
            biome_parameters(&lists, &reports, &biomes, SHA1).unwrap()
        );
        let table = packed::parse(&bytes).unwrap();
        assert_eq!(table.kind, packed::KIND_BIOME_PARAMETERS);
        assert_eq!(table.sections.len(), 2);
        // The overworld is the first section whatever order the lists came in.
        assert_eq!(table.sections[0].rows().len(), 2);
        assert_eq!(table.sections[1].rows().len(), 1);
        let row = table.sections[0].row(0).unwrap();
        let numbers: Vec<i16> = row[..26]
            .chunks(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(
            numbers,
            [
                -10000, 10000, -3500, -1000, -12000, -10500, 0, 0, 10000, 10000, 1, 2, 0
            ]
        );
        assert_eq!(row[26], 2, "plains is the third biome by name");
        assert_eq!(table.sections[0].row(1).unwrap()[26], 1);
        let nether = table.sections[1].row(0).unwrap();
        assert_eq!(i16::from_le_bytes([nether[24], nether[25]]), 1750);
        assert_eq!(nether[26], 0);
    }

    #[test]
    fn a_parameter_that_does_not_fit_sixteen_bits_fails() {
        let (mut lists, mut reports, biomes) = small_lists();
        lists[1].entries[0].bounds[2][0] = -40000;
        reports[0].entries[0].bounds[2][0] = -4.0;
        let error = biome_parameters(&lists, &reports, &biomes, SHA1).unwrap_err();
        assert!(format!("{error:#}").contains("does not fit 16 bits"));
    }

    #[test]
    fn a_parameter_the_report_does_not_confirm_fails() {
        let (lists, mut reports, biomes) = small_lists();
        reports[0].entries[1].offset = 0.3751;
        let error = biome_parameters(&lists, &reports, &biomes, SHA1).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("does not confirm"), "{message}");
        assert!(message.contains("minecraft:desert"), "{message}");

        let (lists, mut reports, biomes) = small_lists();
        reports[1].entries[0].biome = "minecraft:desert".to_owned();
        assert!(biome_parameters(&lists, &reports, &biomes, SHA1).is_err());
        let (lists, mut reports, biomes) = small_lists();
        reports[0].entries.pop();
        assert!(biome_parameters(&lists, &reports, &biomes, SHA1).is_err());
    }

    #[test]
    fn the_report_confirms_every_integer_forwards_where_backwards_would_not() {
        // Going from the report's decimal back to an integer fails on right values;
        // the check here never does, for any integer the game can hold.
        let mut lost_backwards = 0;
        for number in -25_000i64..=25_000 {
            let printed = f64::from(number as f32 / 10000.0);
            assert!(confirms(number, printed), "{number}");
            if (printed as f32 * 10000.0) as i64 != number {
                lost_backwards += 1;
            }
        }
        assert_eq!(lost_backwards, 3182, "ADR-0019 counted these");
        assert!(!confirms(3750, 0.3751));
    }

    #[test]
    fn a_list_the_layout_has_no_section_for_fails() {
        let (mut lists, reports, biomes) = small_lists();
        lists.push(ParameterList {
            name: "the_end".to_owned(),
            entries: Vec::new(),
        });
        assert!(biome_parameters(&lists, &reports, &biomes, SHA1).is_err());
        let (mut lists, reports, biomes) = small_lists();
        lists.remove(0);
        assert!(biome_parameters(&lists, &reports, &biomes, SHA1).is_err());
    }

    #[test]
    fn every_listed_block_says_what_is_done_about_it() {
        for listed in BEYOND_ITS_STATE {
            assert!(!listed.consulted.is_empty(), "{}", listed.block);
            assert!(listed.done.contains("The flag says so"), "{}", listed.block);
        }
    }
}
