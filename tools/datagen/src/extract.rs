//! Reads what the extract program (`tools/datagen/java/Extract.java`) wrote.
//!
//! The program writes text files, one row a line with the fields separated by tabs.
//! Integers are decimal and a double is its bits in hexadecimal, so that nothing rests
//! on how a JVM prints a fraction.

use std::collections::BTreeMap;
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
    /// One for each item, by item id.
    pub items: Vec<ItemDump>,
    /// The biome parameter lists the game knows, sorted by name.
    pub biome_parameters: Vec<ParameterList>,
    /// What the game's noise routers give at fixed positions, by the name of the file
    /// (`overworld_13579`: the noise settings and the seed), sorted.
    pub router_values: Vec<(String, RouterValues)>,
    /// The passes of the game's search for a place to spawn from the climate, in the
    /// game's order.
    pub spawn_rings: Vec<SpawnPass>,
    /// The order in which the game's generators place features.
    pub feature_order: FeatureOrder,
    /// What an entity meets on each block, by block id.
    pub block_physics: Vec<BlockPhysicsDump>,
    /// The distinct collision shapes in the game's voxel form; a shape's index is its
    /// place here.
    pub voxel_shapes: Vec<VoxelShapeDump>,
    /// The collision shapes of each state, by state id.
    pub state_collisions: Vec<StateCollisionDump>,
    /// One for each entity type, by its id.
    pub entity_types: Vec<EntityTypeDump>,
}

/// What an entity meets on a block: the floats the block holds, as their bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockPhysicsDump {
    pub name: String,
    pub friction: u32,
    pub speed_factor: u32,
    pub jump_factor: u32,
    pub bounce_restitution: u32,
    pub fall_distance_reduction: u32,
    /// The game's `OffsetType`: `NONE`, `XZ` or `XYZ`.
    pub offset_type: String,
    pub max_horizontal_offset: u32,
    pub max_vertical_offset: u32,
    /// The name of the block's loot table, which may be another block's.
    pub loot_table: Option<String>,
}

/// A collision shape as the game holds it: the boundaries of its cells along each
/// axis and which cells are filled.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VoxelShapeDump {
    /// Whether it is the instance `Shapes.block()`, which the game tests by identity.
    pub is_block: bool,
    /// The bits of the coordinates along x, y and z, one more than the cells.
    pub coords: [Vec<u64>; 3],
    /// The numbers of cells along x, y and z.
    pub sizes: [u32; 3],
    /// The filled cells as a bit set: bit `(x * y_size + y) * z_size + z`.
    pub cells: Vec<u8>,
}

impl VoxelShapeDump {
    pub fn is_full(&self, x: u32, y: u32, z: u32) -> bool {
        let index = ((x * self.sizes[1] + y) * self.sizes[2] + z) as usize;
        self.cells
            .get(index / 8)
            .is_some_and(|byte| byte & (1 << (index % 8)) != 0)
    }
}

/// The collision shapes of a state, by their indices among the voxel shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateCollisionDump {
    pub collision: usize,
    /// The game's `hasLargeCollisionShape`.
    pub large: bool,
    /// The entity-inside shape; `None` where it is the entity's own collision shape,
    /// or the whole block where that is empty (powder snow).
    pub inside: Option<usize>,
}

/// What the game's `EntityType` says of one type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityTypeDump {
    pub name: String,
    /// Float bits.
    pub width: u32,
    pub height: u32,
    pub eye_height: u32,
    pub fixed: bool,
    pub client_tracking_range: i32,
    pub update_interval: i32,
    pub track_deltas: bool,
    pub fire_immune: bool,
    /// The serialised name of its `MobCategory`, such as `misc`.
    pub category: String,
}

/// The order in which the game places the features of each dimension: by dimension,
/// for each step of decoration, the names of the placed features in the order of the
/// step, which is read from the generators' own `featuresPerStep`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureOrder {
    pub dimensions: BTreeMap<String, Vec<Vec<String>>>,
}

/// One pass of `NoiseSpawnFinder.radialSearch`: the offsets from the ring's centre at
/// which it samples, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnPass {
    pub increment: u32,
    pub offsets: Vec<[i32; 2]>,
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
    /// What a click or a ray hits, without a plant's offset.
    pub outline: Vec<ShapeBox>,
    /// What redstone asks of the state: bit 0 `isRedstoneConductor`, bit 1
    /// `isSignalSource`, bit 2 `hasAnalogOutputSignal`, bit 3 that it cannot be broken
    /// (`getDestroySpeed` is -1), which a piston does not move.
    pub redstone: u8,
    /// The block's `SoundType`: its volume and pitch, and its break, step, place, hit
    /// and fall sounds by their ids in the sound event registry.
    pub sound: SoundTypeDump,
    /// The questions whose answer looked at the level or changed with the position,
    /// such as `collision:block_entity`. Empty for a state that answers by itself.
    pub consulted: Vec<String>,
}

/// What the game's code says of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemDump {
    pub name: String,
    /// The simple name of the item's Java class.
    pub class: String,
    /// The id of the block the item places, if it places one.
    pub block: Option<usize>,
    /// For an item that places a standing or a wall block, the id of the wall block
    /// and the direction it attaches in, as the game names it (`down`, `north`).
    pub wall: Option<(usize, String)>,
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
    /// Whether the block's class says where it can stay (the game's `canSurvive`).
    pub survival_rule: bool,
    /// The simple names of the classes the block's class is made from, nearest first,
    /// without the base of all blocks.
    pub ancestors: Vec<String>,
    /// For a flower pot, the id of the block it holds (air for the empty pot).
    pub potted: Option<usize>,
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
            items: items(&read("items.txt")?).context("reading items.txt")?,
            biome_parameters,
            router_values,
            spawn_rings: spawn_rings(&read("spawn_rings.txt")?)
                .context("reading spawn_rings.txt")?,
            feature_order: feature_order(&read("feature_order.txt")?)
                .context("reading feature_order.txt")?,
            block_physics: block_physics(&read("block_physics.txt")?)
                .context("reading block_physics.txt")?,
            voxel_shapes: voxel_shapes(&read("voxel_shapes.txt")?)
                .context("reading voxel_shapes.txt")?,
            state_collisions: state_collisions(&read("state_collisions.txt")?)
                .context("reading state_collisions.txt")?,
            entity_types: entity_types(&read("entity_types.txt")?)
                .context("reading entity_types.txt")?,
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
        fields.len() == 25,
        "{} fields where 25 are expected",
        fields.len()
    );
    ensure!(
        integer::<usize>(fields[0])? == expected_id,
        "the state ids have a gap"
    );
    let flags: u16 = integer(fields[4])?;
    ensure!(
        flags & !(((1 << 12) - 1) | (1 << 15)) == 0,
        "more flags than the thirteen the row has"
    );

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
        outline: shape(fields[21])?,
        redstone: {
            let redstone: u8 = integer(fields[22])?;
            ensure!(redstone < 1 << 4, "more redstone flags than the four asked");
            redstone
        },
        sound: sound_type(fields[23])?,
        consulted: match fields[24] {
            "-" => Vec::new(),
            listed => listed.split(',').map(str::to_owned).collect(),
        },
    })
}

/// A block's `SoundType` as the program prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SoundTypeDump {
    /// The bits of the volume and the pitch, floats as the game keeps them.
    pub volume: u32,
    pub pitch: u32,
    pub break_sound: u32,
    pub step: u32,
    pub place: u32,
    pub hit: u32,
    pub fall: u32,
}

/// `volume,pitch,break,step,place,hit,fall`: the two floats' bits in hexadecimal, the
/// sounds' ids in decimal.
fn sound_type(text: &str) -> Result<SoundTypeDump> {
    let parts: Vec<&str> = text.split(',').collect();
    ensure!(parts.len() == 7, "a sound type of {} parts", parts.len());
    let bits = |part: &str| u32::from_str_radix(part, 16).context("a float's bits");
    Ok(SoundTypeDump {
        volume: bits(parts[0])?,
        pitch: bits(parts[1])?,
        break_sound: integer(parts[2])?,
        step: integer(parts[3])?,
        place: integer(parts[4])?,
        hit: integer(parts[5])?,
        fall: integer(parts[6])?,
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
        // Fields 5 and 6 say how "used by a click" came about, for a reader.
        ensure!(
            fields.len() == 10,
            "a line has {} fields where 10 are expected",
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
            survival_rule: fields[7] != "-",
            ancestors: match fields[8] {
                "-" => Vec::new(),
                listed => listed.split(',').map(str::to_owned).collect(),
            },
            potted: match integer::<i64>(fields[9])? {
                -1 => None,
                id => Some(usize::try_from(id)?),
            },
        });
    }
    Ok(blocks)
}

/// The lines of `items.txt`.
pub fn items(text: &str) -> Result<Vec<ItemDump>> {
    let mut items = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        ensure!(
            fields.len() == 6,
            "a line has {} fields where 6 are expected",
            fields.len()
        );
        ensure!(
            integer::<usize>(fields[0])? == items.len(),
            "the item ids have a gap at {}",
            fields[0]
        );
        let block: i64 = integer(fields[3])?;
        let wall: i64 = integer(fields[4])?;
        items.push(ItemDump {
            name: fields[1].to_owned(),
            class: fields[2].to_owned(),
            block: if block < 0 {
                None
            } else {
                Some(usize::try_from(block)?)
            },
            wall: if wall < 0 {
                ensure!(fields[5] == "-", "a direction without a wall block");
                None
            } else {
                Some((usize::try_from(wall)?, fields[5].to_owned()))
            },
        });
    }
    Ok(items)
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

/// Lines of a pass's increment, x and z; a pass is the lines with one increment in a
/// row.
/// Reads `feature_order.txt`: one line a feature, its dimension, its step and its name,
/// each dimension's in the order of its steps and each step's in the game's order.
pub fn feature_order(text: &str) -> Result<FeatureOrder> {
    let mut order = FeatureOrder::default();
    for line in text.lines() {
        let [dimension, step, name] = line.split('\t').collect::<Vec<_>>()[..] else {
            bail!("a line of the order of features has three fields: {line:?}");
        };
        let step: usize = step
            .parse()
            .with_context(|| format!("the step of {line:?} is not a number"))?;
        let steps = order.dimensions.entry(dimension.to_string()).or_default();
        ensure!(
            step + 1 >= steps.len(),
            "{name} comes in step {step} of {dimension} after a later step"
        );
        steps.resize_with(steps.len().max(step + 1), Vec::new);
        steps[step].push(name.to_string());
    }
    ensure!(
        !order.dimensions.is_empty(),
        "the order of features is empty"
    );
    Ok(order)
}

pub fn spawn_rings(text: &str) -> Result<Vec<SpawnPass>> {
    let mut passes: Vec<SpawnPass> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        ensure!(
            fields.len() == 3,
            "a line has {} fields where 3 are expected",
            fields.len()
        );
        let increment = integer(fields[0])?;
        let offset = [integer(fields[1])?, integer(fields[2])?];
        match passes.last_mut() {
            Some(pass) if pass.increment == increment => pass.offsets.push(offset),
            _ => {
                ensure!(
                    passes.iter().all(|pass| pass.increment != increment),
                    "the pass of increment {increment} is not all in one place"
                );
                passes.push(SpawnPass {
                    increment,
                    offsets: vec![offset],
                });
            }
        }
    }
    ensure!(!passes.is_empty(), "there are no offsets");
    Ok(passes)
}

/// The fields of a line separated by tabs, `count` of them, the first being the id
/// that `expected_id` has to be.
fn numbered_fields(line: &str, count: usize, expected_id: usize) -> Result<Vec<&str>> {
    let fields: Vec<&str> = line.split('\t').collect();
    ensure!(
        fields.len() == count,
        "a line has {} fields where {count} are expected",
        fields.len()
    );
    ensure!(
        integer::<usize>(fields[0])? == expected_id,
        "the ids have a gap at {}",
        fields[0]
    );
    Ok(fields)
}

/// The bits of a float, in hexadecimal.
fn float_bits(text: &str) -> Result<u32> {
    u32::from_str_radix(text, 16).with_context(|| format!("{text:?} is not a float's bits"))
}

/// The lines of `block_physics.txt`.
pub fn block_physics(text: &str) -> Result<Vec<BlockPhysicsDump>> {
    let mut blocks = Vec::new();
    for line in text.lines() {
        let fields = numbered_fields(line, 11, blocks.len())?;
        ensure!(
            ["NONE", "XZ", "XYZ"].contains(&fields[7]),
            "{:?} is no offset type",
            fields[7]
        );
        blocks.push(BlockPhysicsDump {
            name: fields[1].to_owned(),
            friction: float_bits(fields[2])?,
            speed_factor: float_bits(fields[3])?,
            jump_factor: float_bits(fields[4])?,
            bounce_restitution: float_bits(fields[5])?,
            fall_distance_reduction: float_bits(fields[6])?,
            offset_type: fields[7].to_owned(),
            max_horizontal_offset: float_bits(fields[8])?,
            max_vertical_offset: float_bits(fields[9])?,
            loot_table: match fields[10] {
                "-" => None,
                name => Some(name.to_owned()),
            },
        });
    }
    Ok(blocks)
}

/// The lines of `voxel_shapes.txt`.
pub fn voxel_shapes(text: &str) -> Result<Vec<VoxelShapeDump>> {
    let mut shapes = Vec::new();
    for line in text.lines() {
        let shape = voxel_shape(line).with_context(|| format!("in shape {}", shapes.len()))?;
        shapes.push(shape);
    }
    Ok(shapes)
}

fn voxel_shape(line: &str) -> Result<VoxelShapeDump> {
    let fields: Vec<&str> = line.split('\t').collect();
    let [is_block, xs, ys, zs, sizes, cells] = fields[..] else {
        bail!("a line has {} fields where 6 are expected", fields.len());
    };
    let coords = |text: &str| -> Result<Vec<u64>> {
        text.split(',')
            .map(|bits| u64::from_str_radix(bits, 16).with_context(|| format!("{bits:?}")))
            .collect()
    };
    let sizes: Vec<u32> = sizes.split(',').map(integer).collect::<Result<_>>()?;
    let sizes: [u32; 3] = sizes
        .try_into()
        .map_err(|_| anyhow::anyhow!("a shape has no three sizes"))?;
    let coords = [coords(xs)?, coords(ys)?, coords(zs)?];
    for (axis, list) in coords.iter().enumerate() {
        ensure!(
            list.len() == sizes[axis] as usize + 1,
            "{} coordinates along axis {axis} for {} cells",
            list.len(),
            sizes[axis]
        );
    }
    let count = sizes.iter().map(|&size| size as usize).product::<usize>();
    let cells = if cells == "-" {
        vec![0; count.div_ceil(8)]
    } else {
        ensure!(cells.len() % 2 == 0, "the cells are not whole bytes");
        (0..cells.len() / 2)
            .map(|at| u8::from_str_radix(&cells[2 * at..2 * at + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .context("the cells are not hexadecimal")?
    };
    ensure!(
        cells.len() == count.div_ceil(8),
        "{} bytes of cells for {count} cells",
        cells.len()
    );
    Ok(VoxelShapeDump {
        is_block: boolean(is_block)?,
        coords,
        sizes,
        cells,
    })
}

/// The lines of `state_collisions.txt`.
pub fn state_collisions(text: &str) -> Result<Vec<StateCollisionDump>> {
    let mut states = Vec::new();
    for line in text.lines() {
        let fields = numbered_fields(line, 4, states.len())?;
        states.push(StateCollisionDump {
            collision: integer(fields[1])?,
            large: boolean(fields[2])?,
            inside: match fields[3] {
                "entity" => None,
                index => Some(integer(index)?),
            },
        });
    }
    Ok(states)
}

/// The lines of `entity_types.txt`.
pub fn entity_types(text: &str) -> Result<Vec<EntityTypeDump>> {
    let mut types = Vec::new();
    for line in text.lines() {
        let fields = numbered_fields(line, 11, types.len())?;
        types.push(EntityTypeDump {
            name: fields[1].to_owned(),
            width: float_bits(fields[2])?,
            height: float_bits(fields[3])?,
            eye_height: float_bits(fields[4])?,
            fixed: boolean(fields[5])?,
            client_tracking_range: integer(fields[6])?,
            update_interval: integer(fields[7])?,
            track_deltas: boolean(fields[8])?,
            fire_immune: boolean(fields[9])?,
            category: fields[10].to_owned(),
        });
    }
    Ok(types)
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
             -\t{full}\t{lower}\t{lower}\t{lower}\t{lower}\t{lower};{full}\t{lower}\t\
             5\t3f800000,3f800000,1,2,3,4,5\tcollision:block_entity"
        );
        let read = states(&line).unwrap();
        assert_eq!(read.len(), 1);
        let state = &read[0];
        assert_eq!(state.block, "minecraft:thing");
        assert_eq!((state.light_emission, state.light_dampening), (7, 1));
        assert_eq!(state.flags, 2086);
        assert_eq!(state.sturdy, [63, 2, 1]);
        assert_eq!(state.push_reaction, "POPPED");
        assert_eq!(state.redstone, 5, "a conductor with an analog output");
        assert_eq!((state.sound.place, state.sound.fall), (3, 5));
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
        assert_eq!(state.outline.len(), 1);
        assert_eq!(state.consulted, ["collision:block_entity"]);
    }

    #[test]
    fn a_state_out_of_order_or_with_a_field_missing_is_refused() {
        let line = "1\tminecraft:thing\t0\t0\t0\t0\t0\t0\tPUSH\t-1\t0\t0\t0\tnone\t-\t-\t-\t-\t-\t-\t-\t-\t-";
        assert!(states(line).is_err(), "the first id has to be 0");
        let line =
            "0\tminecraft:thing\t0\t0\t0\t0\t0\t0\tPUSH\t-1\t0\t0\t0\tnone\t-\t-\t-\t-\t-\t-\t-\t-";
        assert!(states(line).is_err(), "a field is missing");
    }

    #[test]
    fn lines_of_blocks_and_of_parameters_are_read() {
        let read = blocks(
            "0\tminecraft:air\tAirBlock\t-1\t0\t-\t-\t-\t-\t-1\n\
             1\tminecraft:chest\tChestBlock\t1\t1\tChestBlock\t-\t-\t\
             AbstractChestBlock,BaseEntityBlock\t-1\n\
             2\tminecraft:potted_fern\tFlowerPotBlock\t-1\t1\tFlowerPotBlock\t-\t-\t-\t7\n\
             3\tminecraft:torch\tTorchBlock\t-1\t0\t-\t-\tBaseTorchBlock\tBaseTorchBlock\t-1\n",
        )
        .unwrap();
        assert_eq!(read[1].class, "ChestBlock");
        assert_eq!(read[1].ancestors, ["AbstractChestBlock", "BaseEntityBlock"]);
        assert!(read[0].ancestors.is_empty());
        assert_eq!((read[2].potted, read[1].potted), (Some(7), None));
        assert!(read[3].survival_rule && !read[1].survival_rule);
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

    #[test]
    fn the_offsets_of_a_spawn_search_are_grouped_by_their_pass() {
        let read = spawn_rings("512\t0\t512\n512\t-3\t-511\n32\t0\t32\n").unwrap();
        assert_eq!(
            read,
            [
                SpawnPass {
                    increment: 512,
                    offsets: vec![[0, 512], [-3, -511]],
                },
                SpawnPass {
                    increment: 32,
                    offsets: vec![[0, 32]],
                },
            ]
        );
        assert!(spawn_rings("512\t0\t512\n32\t0\t32\n512\t1\t1\n").is_err());
        assert!(spawn_rings("").is_err());
        assert!(spawn_rings("512\t0\n").is_err());
    }

    #[test]
    fn the_order_of_features_is_read_by_dimension_and_step_in_the_files_order() {
        let read = feature_order(
            "end\t0\tminecraft:end_island\noverworld\t1\tminecraft:lake\n\
             overworld\t1\tminecraft:a\noverworld\t3\tminecraft:b\n",
        )
        .unwrap();
        assert_eq!(
            read.dimensions["end"],
            [vec!["minecraft:end_island".to_string()]]
        );
        assert_eq!(
            read.dimensions["overworld"],
            [
                vec![],
                vec!["minecraft:lake".to_string(), "minecraft:a".to_string()],
                vec![],
                vec!["minecraft:b".to_string()],
            ]
        );
        assert!(feature_order("overworld\t3\ta\noverworld\t1\tb\n").is_err());
        assert!(feature_order("overworld\t1\n").is_err());
        assert!(feature_order("").is_err());
    }

    #[test]
    fn lines_of_block_physics_are_read_with_their_floats_as_bits() {
        let read = block_physics(
            "0\tminecraft:stone\t3f19999a\t3f800000\t3f800000\t0\t0\tNONE\t0\t0\tminecraft:blocks/stone\n\
             1\tminecraft:bamboo\t3f19999a\t3f800000\t3f800000\t0\t0\tXZ\t3e800000\t0\t-\n",
        )
        .unwrap();
        assert_eq!(f32::from_bits(read[0].friction), 0.6);
        assert_eq!(f32::from_bits(read[0].speed_factor), 1.0);
        assert_eq!(
            read[0].loot_table.as_deref(),
            Some("minecraft:blocks/stone")
        );
        assert_eq!(read[1].offset_type, "XZ");
        assert_eq!(f32::from_bits(read[1].max_horizontal_offset), 0.25);
        assert_eq!(read[1].loot_table, None);
        assert!(block_physics("0\tminecraft:stone\t0\t0\t0\t0\t0\tSIDEWAYS\t0\t0\t-\n").is_err());
        assert!(block_physics("1\tminecraft:stone\t0\t0\t0\t0\t0\tNONE\t0\t0\t-\n").is_err());
    }

    #[test]
    fn a_voxel_shape_is_read_with_its_coordinates_and_cells() {
        let half = "3fe0000000000000";
        // A slab: one cell along x and z, two along y of which the lower is filled.
        let line = format!(
            "0\t0,{ONE}\t0,{half},{ONE}\t0,{ONE}\t1,2,1\t01\n1\t0,{ONE}\t0,{ONE}\t0,{ONE}\t1,1,1\t01\n0\t0\t0\t0\t0,0,0\t-"
        );
        let read = voxel_shapes(&line).unwrap();
        assert_eq!(read.len(), 3);
        assert!(!read[0].is_block && read[1].is_block);
        assert_eq!(read[0].sizes, [1, 2, 1]);
        assert_eq!(read[0].coords[1][1], 0.5f64.to_bits());
        assert!(read[0].is_full(0, 0, 0) && !read[0].is_full(0, 1, 0));
        assert!(read[2].cells.is_empty());
        // Coordinates that do not match the cells are refused.
        assert!(voxel_shapes(&format!("0\t0,{ONE}\t0,{ONE}\t0,{ONE}\t1,2,1\t01")).is_err());
        assert!(voxel_shapes(&format!("0\t0,{ONE}\t0,{ONE}\t0,{ONE}\t1,1,1\t0102")).is_err());
    }

    #[test]
    fn lines_of_state_collisions_and_entity_types_are_read() {
        let read = state_collisions("0\t0\t0\t1\n1\t2\t1\tentity\n").unwrap();
        assert_eq!(
            read,
            [
                StateCollisionDump {
                    collision: 0,
                    large: false,
                    inside: Some(1)
                },
                StateCollisionDump {
                    collision: 2,
                    large: true,
                    inside: None
                },
            ]
        );
        assert!(state_collisions("0\t0\t2\t1\n").is_err());

        let read =
            entity_types("0\tminecraft:item\t3e800000\t3e800000\t3e59999a\t0\t6\t20\t1\t0\tmisc\n")
                .unwrap();
        assert_eq!(read[0].name, "minecraft:item");
        assert_eq!(f32::from_bits(read[0].width), 0.25);
        assert_eq!(f32::from_bits(read[0].eye_height), 0.2125);
        assert_eq!(
            (read[0].client_tracking_range, read[0].update_interval),
            (6, 20)
        );
        assert!(read[0].track_deltas && !read[0].fire_immune && !read[0].fixed);
        assert_eq!(read[0].category, "misc");
    }
}
