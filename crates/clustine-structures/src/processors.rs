// Adapted from SteelMC's `steel-core/src/worldgen/template/processors.rs` and
// `steel-registry/src/structure/processor/data.rs`.

//! The game's structure processors (`StructureProcessor` and the eleven types of
//! `StructureProcessorType`), which change or drop each block of a template on its way
//! into the world, read from the data as the game's codecs read it.
//!
//! The types are every one the game registers: `block_ignore`, `block_rot`, `gravity`,
//! `jigsaw_replacement`, `rule`, `nop`, `block_age`, `blackstone_replace`,
//! `lava_submerged_block`, `protected_blocks` and `capped`. The data of the pinned jar
//! names four of them (`rule`, `protected_blocks`, `block_rot`, `capped`); the game's
//! code adds the others: a pool's single element ignores structure blocks and replaces
//! jigsaws, a pool that matches the terrain adds `gravity`, and ruined portals and ocean
//! ruins make their own lists.
//!
//! A rule's tests are the game's: of the block (`RuleTest`: `always_true`,
//! `block_match`, `blockstate_match`, `tag_match`, `random_block_match`,
//! `random_blockstate_match`, and 26.x's `all_of`, `any_of`, `not` and `height_match`,
//! which only ores use in the data), of the position (`PosRuleTest`: `always_true`,
//! `linear_pos`, `axis_aligned_linear_pos`), and what it does to the block entity's
//! data (`RuleBlockEntityModifier`: `passthrough`, `clear`, `append_static`,
//! `append_loot`). SteelMC lacks `linear_pos`, `clear` and `append_static`, and the
//! processors `block_ignore`, `gravity`, `jigsaw_replacement` and `nop` as data.
//!
//! Each processor draws exactly what the game's draws, from the source the game's
//! does: a rule from `RandomSource.create(Mth.getSeed(pos))` of the block's place in
//! the world, `block_rot` and `block_age` from the placement's own source where it was
//! given one and from the position's otherwise (`StructurePlaceSettings.getRandom`),
//! `capped` from the world's seed at the template's position.

use clustine_data::{BlockState, blocks};
use clustine_noise::random::get_seed;
use clustine_noise::{LegacyRandom, PositionalRandom, Random};
use clustine_protocol::nbt::Nbt;
use clustine_world::BlockPos;
use clustine_world::generation::{FinalHeightmap, Neighbourhood, WorldgenHeightmap};
use clustine_worldgen_data::feature::Data;

use crate::blocks::{
    BlockSet, block_id, block_tag, in_block_tag, is_block, known, nbt_equal, put, read_state,
};
use crate::reader::{Read, Reader};

/// One block on its way through the processors (`StructureBlockInfo`): where it stands,
/// in the template or in the world, its state and its block entity's data.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockInfo {
    pub pos: BlockPos,
    pub state: BlockState,
    /// A compound.
    pub nbt: Option<Nbt>,
}

impl BlockInfo {
    /// The game's `StructureBlockInfo.equals`: the data is compared as a map.
    pub fn same_as(&self, other: &Self) -> bool {
        self.pos == other.pos
            && self.state == other.state
            && match (&self.nbt, &other.nbt) {
                (Some(a), Some(b)) => nbt_equal(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

/// Where `StructurePlaceSettings.getRandom` draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceRandom {
    /// No source was given (`setRandom` was not called): a fresh one for each position,
    /// `RandomSource.create(Mth.getSeed(pos))`.
    Positional,
    /// The source given to `placeInWorld`, which the caller also gave to `setRandom`
    /// (fossils and the template feature do; pieces of structures do not).
    Given,
}

/// What a processor works with: the world, where the template is placed, and the
/// random sources.
pub struct Processing<'a> {
    pub world: &'a mut dyn Neighbourhood,
    /// Where the template's corner is placed (`targetPosition`).
    pub position: BlockPos,
    /// The reference that position tests measure from (`referencePos`).
    pub reference: BlockPos,
    pub random_kind: PlaceRandom,
    /// The source given to `placeInWorld`.
    pub random: &'a mut dyn Random,
    /// The world's seed, which `capped` draws from.
    pub world_seed: i64,
}

impl Processing<'_> {
    /// Runs `f` with `settings.getRandom(pos)`.
    fn settings_random<R>(&mut self, pos: BlockPos, f: impl FnOnce(&mut dyn Random) -> R) -> R {
        match self.random_kind {
            PlaceRandom::Given => f(&mut *self.random),
            PlaceRandom::Positional => {
                let mut random = position_random(pos);
                f(&mut random)
            }
        }
    }
}

/// `RandomSource.create(Mth.getSeed(pos))`.
pub fn position_random(pos: BlockPos) -> LegacyRandom {
    LegacyRandom::from_seed(get_seed(pos.x, pos.y, pos.z) as u64)
}

/// The game's `Heightmap.Types`, as `gravity` names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heightmap {
    WorldSurfaceWg,
    WorldSurface,
    OceanFloorWg,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

impl Heightmap {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "WORLD_SURFACE_WG" => Self::WorldSurfaceWg,
            "WORLD_SURFACE" => Self::WorldSurface,
            "OCEAN_FLOOR_WG" => Self::OceanFloorWg,
            "OCEAN_FLOOR" => Self::OceanFloor,
            "MOTION_BLOCKING" => Self::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => Self::MotionBlockingNoLeaves,
            _ => return None,
        })
    }

    /// `level.getHeight`: the y above the highest block that counts. The two of world
    /// generation are terrain's, frozen while features are placed.
    pub fn height(self, world: &mut dyn Neighbourhood, x: i32, z: i32) -> i32 {
        match self {
            Self::WorldSurfaceWg => world.worldgen_height(WorldgenHeightmap::WorldSurface, x, z),
            Self::OceanFloorWg => world.worldgen_height(WorldgenHeightmap::OceanFloor, x, z),
            Self::WorldSurface => world.height(FinalHeightmap::WorldSurface, x, z),
            Self::OceanFloor => world.height(FinalHeightmap::OceanFloor, x, z),
            Self::MotionBlocking => world.height(FinalHeightmap::MotionBlocking, x, z),
            Self::MotionBlockingNoLeaves => {
                world.height(FinalHeightmap::MotionBlockingNoLeaves, x, z)
            }
        }
    }
}

/// An `int` drawn by one of the game's `IntProvider`s, of the kinds a processor's
/// limit can be. Only a constant is in the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntProvider {
    Constant(i32),
    Uniform {
        min: i32,
        max: i32,
    },
    BiasedToBottom {
        min: i32,
        max: i32,
    },
    Clamped {
        source: Box<IntProvider>,
        min: i32,
        max: i32,
    },
}

impl IntProvider {
    pub fn read(reader: &Reader) -> Read<Self> {
        if reader.is_number() {
            return Ok(Self::Constant(reader.int()?));
        }
        let int = |key: &str| reader.field(key)?.int();
        let provider = match reader.field("type")?.text()? {
            "minecraft:constant" => Self::Constant(int("value")?),
            "minecraft:uniform" => Self::Uniform {
                min: int("min_inclusive")?,
                max: int("max_inclusive")?,
            },
            "minecraft:biased_to_bottom" => Self::BiasedToBottom {
                min: int("min_inclusive")?,
                max: int("max_inclusive")?,
            },
            "minecraft:clamped" => Self::Clamped {
                source: Box::new(Self::read(&reader.field("source")?)?),
                min: int("min_inclusive")?,
                max: int("max_inclusive")?,
            },
            other => {
                return Err(reader.error(format!("{other} is not an int provider built here")));
            }
        };
        reader.finish()?;
        Ok(provider)
    }

    pub fn sample<R: Random + ?Sized>(&self, random: &mut R) -> i32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform { min, max } => random.next_i32_bounded(max - min + 1) + min,
            Self::BiasedToBottom { min, max } => {
                let bound = random.next_i32_bounded(max - min + 1) + 1;
                min + random.next_i32_bounded(bound)
            }
            Self::Clamped { source, min, max } => source.sample(random).clamp(*min, *max),
        }
    }

    /// `getMaxValue`.
    pub fn max_value(&self) -> i32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform { max, .. } | Self::BiasedToBottom { max, .. } => *max,
            Self::Clamped { source, max, .. } => source.max_value().min(*max),
        }
    }
}

/// A test of one block state (`RuleTest`).
#[derive(Debug, Clone, PartialEq)]
pub enum RuleTest {
    AlwaysTrue,
    /// The block, by its id.
    BlockMatch(usize),
    BlockStateMatch(BlockState),
    /// A block tag, without `#`.
    TagMatch(&'static str),
    RandomBlockMatch(usize, f32),
    RandomBlockStateMatch(BlockState, f32),
    /// The position's y lies in the range, both ends included.
    HeightMatch(i32, i32),
    AllOf(Vec<RuleTest>),
    AnyOf(Vec<RuleTest>),
    Not(Box<RuleTest>),
}

impl RuleTest {
    pub fn read(reader: &Reader) -> Read<Self> {
        let block = |key: &str| -> Read<usize> {
            let field = reader.field(key)?;
            let name = field.text()?;
            clustine_data::block_id_by_name(name)
                .ok_or_else(|| field.error(format!("{name} is not a block")))
        };
        let list = |key: &str| -> Read<Vec<RuleTest>> {
            reader.field(key)?.list()?.iter().map(Self::read).collect()
        };
        let test = match reader.field("predicate_type")?.text()? {
            "minecraft:always_true" => Self::AlwaysTrue,
            "minecraft:block_match" => Self::BlockMatch(block("block")?),
            "minecraft:blockstate_match" => {
                Self::BlockStateMatch(read_state(&reader.field("block_state")?)?)
            }
            "minecraft:tag_match" => {
                let tag = reader.field("tag")?;
                let name = tag.text()?;
                if block_tag(name).is_none() {
                    return Err(tag.error(format!("{name} is not a tag of blocks")));
                }
                Self::TagMatch(name)
            }
            "minecraft:random_block_match" => {
                Self::RandomBlockMatch(block("block")?, reader.field("probability")?.float()?)
            }
            "minecraft:random_blockstate_match" => Self::RandomBlockStateMatch(
                read_state(&reader.field("block_state")?)?,
                reader.field("probability")?.float()?,
            ),
            "minecraft:height_match" => Self::HeightMatch(
                reader.field("min_inclusive")?.int()?,
                reader.field("max_inclusive")?.int()?,
            ),
            "minecraft:all_of" => Self::AllOf(list("rules")?),
            "minecraft:any_of" => Self::AnyOf(list("rules")?),
            "minecraft:not" => Self::Not(Box::new(Self::read(&reader.field("rule")?)?)),
            other => return Err(reader.error(format!("{other} is not a rule test"))),
        };
        reader.finish()?;
        Ok(test)
    }

    /// Whether `state`, at height `y`, passes. Only the random tests draw, and only
    /// once their block has matched.
    pub fn test<R: Random + ?Sized>(&self, state: BlockState, y: i32, random: &mut R) -> bool {
        match self {
            Self::AlwaysTrue => true,
            Self::BlockMatch(block) => block_id(state) == *block,
            Self::BlockStateMatch(wanted) => state == *wanted,
            Self::TagMatch(tag) => in_block_tag(state, tag),
            Self::RandomBlockMatch(block, probability) => {
                block_id(state) == *block && random.next_f32() < *probability
            }
            Self::RandomBlockStateMatch(wanted, probability) => {
                state == *wanted && random.next_f32() < *probability
            }
            Self::HeightMatch(min, max) => (*min..=*max).contains(&y),
            Self::AllOf(all) => all.iter().all(|test| test.test(state, y, random)),
            Self::AnyOf(any) => any.iter().any(|test| test.test(state, y, random)),
            Self::Not(test) => !test.test(state, y, random),
        }
    }
}

/// The axis `axis_aligned_linear_pos` measures along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// A test of a block's place (`PosRuleTest`).
#[derive(Debug, Clone, PartialEq)]
pub enum PosRuleTest {
    AlwaysTrue,
    /// The chance falls from `min_chance` to `max_chance` between `min_dist` and
    /// `max_dist` blocks from the reference, counted as the game's `distManhattan`.
    LinearPos {
        min_chance: f32,
        max_chance: f32,
        min_dist: i32,
        max_dist: i32,
    },
    /// The same, counted along one axis.
    AxisAlignedLinearPos {
        axis: Axis,
        min_chance: f32,
        max_chance: f32,
        min_dist: i32,
        max_dist: i32,
    },
}

impl PosRuleTest {
    pub fn read(reader: &Reader) -> Read<Self> {
        let float = |key: &str| -> Read<f32> {
            reader.optional(key).map_or(Ok(0.0), |value| value.float())
        };
        let int =
            |key: &str| -> Read<i32> { reader.optional(key).map_or(Ok(0), |value| value.int()) };
        let test = match reader.field("predicate_type")?.text()? {
            "minecraft:always_true" => Self::AlwaysTrue,
            "minecraft:linear_pos" => Self::LinearPos {
                min_chance: float("min_chance")?,
                max_chance: float("max_chance")?,
                min_dist: int("min_dist")?,
                max_dist: int("max_dist")?,
            },
            "minecraft:axis_aligned_linear_pos" => Self::AxisAlignedLinearPos {
                axis: match reader.optional("axis") {
                    None => Axis::Y,
                    Some(axis) => match axis.text()? {
                        "x" => Axis::X,
                        "y" => Axis::Y,
                        "z" => Axis::Z,
                        other => return Err(axis.error(format!("{other} is not an axis"))),
                    },
                },
                min_chance: float("min_chance")?,
                max_chance: float("max_chance")?,
                min_dist: int("min_dist")?,
                max_dist: int("max_dist")?,
            },
            other => return Err(reader.error(format!("{other} is not a position rule test"))),
        };
        reader.finish()?;
        Ok(test)
    }

    /// Whether the block at `world` passes, measured from `reference`. The two linear
    /// tests draw one float each, after the distance is known.
    pub fn test<R: Random + ?Sized>(
        &self,
        world: BlockPos,
        reference: BlockPos,
        random: &mut R,
    ) -> bool {
        let (distance, min_chance, max_chance, min_dist, max_dist) = match *self {
            Self::AlwaysTrue => return true,
            Self::LinearPos {
                min_chance,
                max_chance,
                min_dist,
                max_dist,
            } => {
                let distance = (world.x - reference.x).abs()
                    + (world.y - reference.y).abs()
                    + (world.z - reference.z).abs();
                (distance, min_chance, max_chance, min_dist, max_dist)
            }
            Self::AxisAlignedLinearPos {
                axis,
                min_chance,
                max_chance,
                min_dist,
                max_dist,
            } => {
                let distance = match axis {
                    Axis::X => (world.x - reference.x).abs(),
                    Axis::Y => (world.y - reference.y).abs(),
                    Axis::Z => (world.z - reference.z).abs(),
                };
                (distance, min_chance, max_chance, min_dist, max_dist)
            }
        };
        let draw = random.next_f32();
        draw <= clamped_lerp(
            inverse_lerp(distance as f32, min_dist as f32, max_dist as f32),
            min_chance,
            max_chance,
        )
    }
}

/// `Mth.inverseLerp` in single precision. Equal ends divide by nought as Java does:
/// to an infinity, or to not-a-number at the end itself, which no draw is at or below.
fn inverse_lerp(value: f32, min: f32, max: f32) -> f32 {
    (value - min) / (max - min)
}

/// `Mth.clampedLerp` in single precision.
fn clamped_lerp(factor: f32, min: f32, max: f32) -> f32 {
    if factor < 0.0 {
        min
    } else if factor > 1.0 {
        max
    } else {
        min + factor * (max - min)
    }
}

/// What a rule does to the block entity's data (`RuleBlockEntityModifier`).
#[derive(Debug, Clone, PartialEq)]
pub enum BlockEntityModifier {
    /// The data as it was, or none.
    Passthrough,
    /// An empty compound in place of the data.
    Clear,
    /// The data, or an empty compound, with `data` merged in.
    AppendStatic(Nbt),
    /// The data, or an empty compound, with a loot table and a seed drawn for it.
    AppendLoot(String),
}

impl BlockEntityModifier {
    pub fn read(reader: &Reader) -> Read<Self> {
        let modifier = match reader.field("type")?.text()? {
            "minecraft:passthrough" => Self::Passthrough,
            "minecraft:clear" => Self::Clear,
            "minecraft:append_static" => {
                let data = reader.field("data")?;
                if !data.is_object() {
                    return Err(data.error("is not a compound"));
                }
                Self::AppendStatic(data_to_nbt(data.data()))
            }
            "minecraft:append_loot" => {
                Self::AppendLoot(reader.field("loot_table")?.text()?.to_string())
            }
            other => return Err(reader.error(format!("{other} is not a block entity modifier"))),
        };
        reader.finish()?;
        Ok(modifier)
    }

    /// `apply(random, existingTag)`.
    pub fn apply<R: Random + ?Sized>(&self, random: &mut R, existing: Option<Nbt>) -> Option<Nbt> {
        let copied = || match &existing {
            Some(Nbt::Compound(members)) => members.clone(),
            _ => Vec::new(),
        };
        match self {
            Self::Passthrough => existing,
            Self::Clear => Some(Nbt::Compound(Vec::new())),
            Self::AppendStatic(data) => {
                let mut members = copied();
                merge(&mut members, data);
                Some(Nbt::Compound(members))
            }
            Self::AppendLoot(table) => {
                let mut members = copied();
                put(&mut members, "LootTable", Nbt::String(table.clone()));
                put(&mut members, "LootTableSeed", Nbt::Long(random.next_i64()));
                Some(Nbt::Compound(members))
            }
        }
    }
}

/// `CompoundTag.merge`: compounds merge into compounds, anything else replaces.
fn merge(members: &mut Vec<(String, Nbt)>, other: &Nbt) {
    let Nbt::Compound(others) = other else {
        return;
    };
    for (key, value) in others {
        match (members.iter_mut().find(|(name, _)| name == key), value) {
            (Some((_, Nbt::Compound(inner))), Nbt::Compound(_)) => merge(inner, value),
            _ => put(members, key, value.clone()),
        }
    }
}

/// A value of the data as the game's `JsonOps` turns it into a tag: a whole number into
/// the narrowest of byte, short, int and long, another into a float where that loses
/// nothing and a double otherwise, a truth value into a byte.
pub fn data_to_nbt(data: &Data) -> Nbt {
    match data {
        Data::Bool(value) => Nbt::Byte(i8::from(*value)),
        Data::Number(text) => {
            if let Ok(whole) = text.parse::<i64>() {
                if let Ok(byte) = i8::try_from(whole) {
                    Nbt::Byte(byte)
                } else if let Ok(short) = i16::try_from(whole) {
                    Nbt::Short(short)
                } else if let Ok(int) = i32::try_from(whole) {
                    Nbt::Int(int)
                } else {
                    Nbt::Long(whole)
                }
            } else {
                let double: f64 = text.parse().unwrap_or(f64::NAN);
                if f64::from(double as f32) == double {
                    Nbt::Float(double as f32)
                } else {
                    Nbt::Double(double)
                }
            }
        }
        Data::Text(text) => Nbt::String((*text).to_string()),
        Data::List(items) => Nbt::List(items.iter().map(data_to_nbt).collect()),
        Data::Object(members) => Nbt::Compound(
            members
                .iter()
                .map(|(key, value)| ((*key).to_string(), data_to_nbt(value)))
                .collect(),
        ),
    }
}

/// One rule of a `rule` processor (`ProcessorRule`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorRule {
    pub input: RuleTest,
    pub location: RuleTest,
    pub position: PosRuleTest,
    pub output: BlockState,
    pub modifier: BlockEntityModifier,
}

impl ProcessorRule {
    pub fn read(reader: &Reader) -> Read<Self> {
        let rule = Self {
            input: RuleTest::read(&reader.field("input_predicate")?)?,
            location: RuleTest::read(&reader.field("location_predicate")?)?,
            position: match reader.optional("position_predicate") {
                Some(test) => PosRuleTest::read(&test)?,
                None => PosRuleTest::AlwaysTrue,
            },
            output: read_state(&reader.field("output_state")?)?,
            modifier: match reader.optional("block_entity_modifier") {
                Some(modifier) => BlockEntityModifier::read(&modifier)?,
                None => BlockEntityModifier::Passthrough,
            },
        };
        reader.finish()?;
        Ok(rule)
    }
}

/// A structure processor.
#[derive(Debug, Clone, PartialEq)]
pub enum Processor {
    /// Drops the blocks of these block ids (`BlockIgnoreProcessor`).
    BlockIgnore(Vec<usize>),
    /// Drops a block, of the set where one is given, unless a draw is at most the
    /// integrity.
    BlockRot {
        rottable: Option<BlockSet>,
        integrity: f32,
    },
    /// Moves the block to the height of the heightmap in its column, plus the offset,
    /// plus its height in the template.
    Gravity {
        heightmap: Heightmap,
        offset: i32,
    },
    /// A jigsaw becomes the state its data names to become.
    JigsawReplacement,
    /// The first rule that matches replaces the block.
    Rule(Vec<ProcessorRule>),
    Nop,
    /// Ruined portals' weathering: stone bricks crack and moss, obsidian cries.
    BlockAge {
        mossiness: f32,
    },
    /// Stone of a ruined portal in the Nether becomes blackstone.
    BlackstoneReplace,
    /// A block that does not fill its cube becomes lava where the world has lava.
    LavaSubmergedBlock,
    /// Drops a block where the world's block is of the tag (without `#`).
    ProtectedBlocks(&'static str),
    /// The delegate, applied after everything else to at most `limit` blocks it
    /// changes, chosen at random.
    Capped {
        delegate: Box<Processor>,
        limit: IntProvider,
    },
}

/// A list of processors (`StructureProcessorList`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcessorList {
    pub processors: Vec<Processor>,
}

impl ProcessorList {
    /// Reads a list as the game's `StructureProcessorType.LIST_CODEC` does: the name of
    /// a list in the registry, which `named` gives the data of, or a list in place,
    /// either `{"processors": [...]}` or the bare list.
    pub fn read(reader: &Reader, named: &dyn Fn(&str) -> Option<&'static Data>) -> Read<Self> {
        if reader.is_text() {
            let name = reader.text()?;
            let data = named(name)
                .ok_or_else(|| reader.error(format!("{name} is not a processor list")))?;
            return Self::read(&Reader::new(data, name), named);
        }
        let items = if reader.is_list() {
            reader.list()?
        } else {
            let list = reader.field("processors")?.list()?;
            reader.finish()?;
            list
        };
        Ok(Self {
            processors: items.iter().map(Processor::read).collect::<Read<_>>()?,
        })
    }
}

impl Processor {
    /// `BlockIgnoreProcessor.STRUCTURE_BLOCK`, which every pool element adds.
    pub fn structure_block_ignore() -> Self {
        Self::BlockIgnore(vec![block_id(blocks::STRUCTURE_BLOCK)])
    }

    /// `BlockIgnoreProcessor.STRUCTURE_AND_AIR`, which a legacy pool element adds.
    pub fn structure_and_air_ignore() -> Self {
        Self::BlockIgnore(vec![
            block_id(blocks::AIR),
            block_id(blocks::STRUCTURE_BLOCK),
        ])
    }

    /// `BlockIgnoreProcessor.AIR`.
    pub fn air_ignore() -> Self {
        Self::BlockIgnore(vec![block_id(blocks::AIR)])
    }

    /// What a pool that matches the terrain adds (`Projection.TERRAIN_MATCHING`).
    pub fn terrain_matching() -> Self {
        Self::Gravity {
            heightmap: Heightmap::WorldSurfaceWg,
            offset: -1,
        }
    }

    pub fn read(reader: &Reader) -> Read<Self> {
        let processor = match reader.field("processor_type")?.text()? {
            "minecraft:block_ignore" => Self::BlockIgnore(
                reader
                    .field("blocks")?
                    .list()?
                    .iter()
                    .map(|state| read_state(state).map(block_id))
                    .collect::<Read<_>>()?,
            ),
            "minecraft:block_rot" => Self::BlockRot {
                rottable: match reader.optional("rottable_blocks") {
                    Some(set) => Some(BlockSet::read(&set)?),
                    None => None,
                },
                integrity: reader.field("integrity")?.float()?,
            },
            "minecraft:gravity" => Self::Gravity {
                heightmap: match reader.optional("heightmap") {
                    Some(name) => {
                        let text = name.text()?;
                        Heightmap::named(text)
                            .ok_or_else(|| name.error(format!("{text} is not a heightmap")))?
                    }
                    None => Heightmap::WorldSurfaceWg,
                },
                offset: reader
                    .optional("offset")
                    .map_or(Ok(0), |offset| offset.int())?,
            },
            "minecraft:jigsaw_replacement" => Self::JigsawReplacement,
            "minecraft:rule" => Self::Rule(
                reader
                    .field("rules")?
                    .list()?
                    .iter()
                    .map(ProcessorRule::read)
                    .collect::<Read<_>>()?,
            ),
            "minecraft:nop" => Self::Nop,
            "minecraft:block_age" => Self::BlockAge {
                mossiness: reader.field("mossiness")?.float()?,
            },
            "minecraft:blackstone_replace" => Self::BlackstoneReplace,
            "minecraft:lava_submerged_block" => Self::LavaSubmergedBlock,
            "minecraft:protected_blocks" => {
                let value = reader.field("value")?;
                let text = value.text()?;
                let tag = text
                    .strip_prefix('#')
                    .ok_or_else(|| value.error(format!("{text} is not a tag")))?;
                if block_tag(tag).is_none() {
                    return Err(value.error(format!("{tag} is not a tag of blocks")));
                }
                Self::ProtectedBlocks(tag)
            }
            "minecraft:capped" => Self::Capped {
                delegate: Box::new(Self::read(&reader.field("delegate")?)?),
                limit: IntProvider::read(&reader.field("limit")?)?,
            },
            other => return Err(reader.error(format!("{other} is not a structure processor"))),
        };
        reader.finish()?;
        Ok(processor)
    }

    /// `processBlock`: the block as this processor leaves it, or `None` where it drops
    /// it. `original` is the block as the template has it, its position the
    /// template's; `processed` is as the processors before left it, its position the
    /// world's.
    pub fn process_block(
        &self,
        cx: &mut Processing<'_>,
        original: &BlockInfo,
        processed: BlockInfo,
    ) -> Option<BlockInfo> {
        match self {
            Self::BlockIgnore(blocks) => {
                (!blocks.contains(&block_id(processed.state))).then_some(processed)
            }
            Self::BlockRot {
                rottable,
                integrity,
            } => {
                // The draw is made only for a block of the set, which is asked of the
                // block as the processors before left it: a jigsaw already replaced by
                // what it becomes can rot.
                if rottable
                    .as_ref()
                    .is_none_or(|set| set.contains(processed.state))
                    && cx.settings_random(processed.pos, |random| random.next_f32()) > *integrity
                {
                    None
                } else {
                    Some(processed)
                }
            }
            Self::Gravity { heightmap, offset } => {
                let pos = processed.pos;
                let height = heightmap.height(cx.world, pos.x, pos.z) + offset;
                Some(BlockInfo {
                    pos: BlockPos::new(pos.x, height + original.pos.y, pos.z),
                    ..processed
                })
            }
            Self::JigsawReplacement => jigsaw_replacement(processed),
            Self::Rule(rules) => {
                let mut random = position_random(processed.pos);
                let location = cx.world.block(processed.pos);
                for rule in rules {
                    if rule
                        .input
                        .test(processed.state, original.pos.y, &mut random)
                        && rule.location.test(location, processed.pos.y, &mut random)
                        && rule.position.test(processed.pos, cx.reference, &mut random)
                    {
                        let nbt = rule.modifier.apply(&mut random, processed.nbt);
                        return Some(BlockInfo {
                            pos: processed.pos,
                            state: rule.output,
                            nbt,
                        });
                    }
                }
                Some(processed)
            }
            Self::Nop | Self::Capped { .. } => Some(processed),
            Self::BlockAge { mossiness } => {
                let mossiness = *mossiness;
                let state = processed.state;
                let new =
                    cx.settings_random(processed.pos, |random| block_age(state, mossiness, random));
                Some(match new {
                    Some(state) => BlockInfo { state, ..processed },
                    None => processed,
                })
            }
            Self::BlackstoneReplace => Some(blackstone_replace(processed)),
            Self::LavaSubmergedBlock => {
                let lava = is_block(cx.world.block(processed.pos), "minecraft:lava");
                if lava
                    && processed.state.props().outline_shape() != clustine_data::Shape::FULL_BLOCK
                {
                    Some(BlockInfo {
                        state: blocks::LAVA,
                        ..processed
                    })
                } else {
                    Some(processed)
                }
            }
            Self::ProtectedBlocks(tag) => {
                (!in_block_tag(cx.world.block(processed.pos), tag)).then_some(processed)
            }
        }
    }

    /// `finalizeProcessing`, after every block has been through every processor: only
    /// `capped` does anything here.
    pub fn finalize(
        &self,
        cx: &mut Processing<'_>,
        originals: &[BlockInfo],
        mut processed: Vec<BlockInfo>,
    ) -> Vec<BlockInfo> {
        let Self::Capped { delegate, limit } = self else {
            return processed;
        };
        if limit.max_value() == 0 || processed.is_empty() {
            return processed;
        }
        if originals.len() != processed.len() {
            // The game logs this and leaves the list.
            return processed;
        }
        let mut random = LegacyRandom::from_seed(cx.world_seed as u64)
            .next_positional()
            .at(cx.position.x, cx.position.y, cx.position.z);
        let count = i32::try_from(processed.len()).expect("a template has fewer than 2^31 blocks");
        let max_to_replace = limit.sample(&mut random).min(count);
        if max_to_replace < 1 {
            return processed;
        }
        // `Util.toShuffledList`.
        let mut indices: Vec<usize> = (0..processed.len()).collect();
        for i in (2..=indices.len()).rev() {
            let swap_to = random.next_i32_bounded(i as i32) as usize;
            indices.swap(i - 1, swap_to);
        }
        let mut replaced = 0;
        for index in indices {
            let current = processed[index].clone();
            if let Some(altered) = delegate.process_block(cx, &originals[index], current)
                && !processed[index].same_as(&altered)
            {
                replaced += 1;
                processed[index] = altered;
            }
            if replaced >= max_to_replace {
                break;
            }
        }
        processed
    }
}

/// `JigsawReplacementProcessor.processBlock`.
fn jigsaw_replacement(processed: BlockInfo) -> Option<BlockInfo> {
    if !is_block(processed.state, "minecraft:jigsaw") {
        return Some(processed);
    }
    let Some(nbt) = &processed.nbt else {
        // The game warns and leaves the block.
        return Some(processed);
    };
    let text = match nbt.get("final_state") {
        Some(Nbt::String(text)) => text.as_str(),
        _ => "minecraft:air",
    };
    // A state the game cannot read drops the block, with an error in its log; every
    // jigsaw of the pinned jar names one it can.
    let state = parse_block_state(text)?;
    if is_block(state, "minecraft:structure_void") {
        return None;
    }
    Some(BlockInfo {
        pos: processed.pos,
        state,
        nbt: None,
    })
}

/// A state as `BlockStateParser.parseForBlock` reads one with data allowed: a block's
/// name, with or without its namespace, its properties in brackets, and data in braces,
/// which is left out of the state.
pub fn parse_block_state(text: &str) -> Option<BlockState> {
    let text = match text.find('{') {
        Some(at) => &text[..at],
        None => text,
    };
    let text = text.trim();
    if text.contains(':') {
        BlockState::parse(text)
    } else {
        BlockState::parse(&format!("minecraft:{text}"))
    }
}

/// `BlockAgeProcessor`: the new state of a block, or `None` where it stays.
fn block_age(state: BlockState, mossiness: f32, random: &mut dyn Random) -> Option<BlockState> {
    let name = state.block().map_or("", |block| block.name);
    if matches!(
        name,
        "minecraft:stone_bricks" | "minecraft:stone" | "minecraft:chiseled_stone_bricks"
    ) {
        if random.next_f32() >= 0.5 {
            return None;
        }
        let non_mossy = [
            known("minecraft:cracked_stone_bricks"),
            random_facing_stairs(random, "minecraft:stone_brick_stairs"),
        ];
        let mossy = [
            known("minecraft:mossy_stone_bricks"),
            random_facing_stairs(random, "minecraft:mossy_stone_brick_stairs"),
        ];
        return Some(random_block(random, mossiness, non_mossy, mossy));
    }
    if in_block_tag(state, "minecraft:stairs") {
        let facing = state.property("facing").unwrap_or("north");
        let half = state.property("half").unwrap_or("bottom");
        if random.next_f32() >= 0.5 {
            return None;
        }
        let mossy_stairs = known("minecraft:mossy_stone_brick_stairs")
            .with_property("facing", facing)
            .and_then(|stairs| stairs.with_property("half", half))
            .expect("stairs have a facing and a half");
        let non_mossy = [
            known("minecraft:stone_slab"),
            known("minecraft:stone_brick_slab"),
        ];
        let mossy = [mossy_stairs, known("minecraft:mossy_stone_brick_slab")];
        return Some(random_block(random, mossiness, non_mossy, mossy));
    }
    if in_block_tag(state, "minecraft:slabs") {
        return (random.next_f32() < mossiness).then(|| known("minecraft:mossy_stone_brick_slab"));
    }
    if in_block_tag(state, "minecraft:walls") {
        return (random.next_f32() < mossiness).then(|| known("minecraft:mossy_stone_brick_wall"));
    }
    if name == "minecraft:obsidian" {
        return (random.next_f32() < 0.15).then(|| known("minecraft:crying_obsidian"));
    }
    None
}

/// `getRandomFacingStairs`: a facing of the four sideways ones, then a half.
fn random_facing_stairs(random: &mut dyn Random, name: &str) -> BlockState {
    const FACINGS: [&str; 4] = ["north", "east", "south", "west"];
    let facing = FACINGS[random.next_i32_bounded(4) as usize];
    let half = ["top", "bottom"][random.next_i32_bounded(2) as usize];
    known(name)
        .with_property("facing", facing)
        .and_then(|stairs| stairs.with_property("half", half))
        .expect("stairs have a facing and a half")
}

/// `getRandomBlock(random, nonMossy, mossy)`: which of the two, then which of it.
fn random_block(
    random: &mut dyn Random,
    mossiness: f32,
    non_mossy: [BlockState; 2],
    mossy: [BlockState; 2],
) -> BlockState {
    let blocks = if random.next_f32() < mossiness {
        mossy
    } else {
        non_mossy
    };
    blocks[random.next_i32_bounded(2) as usize]
}

/// `BlackstoneReplaceProcessor`: the block's replacement, with its facing, half and
/// slab type where both have them.
fn blackstone_replace(processed: BlockInfo) -> BlockInfo {
    let name = processed.state.block().map_or("", |block| block.name);
    let replacement = match name {
        "minecraft:cobblestone" | "minecraft:mossy_cobblestone" => "minecraft:blackstone",
        "minecraft:stone" => "minecraft:polished_blackstone",
        "minecraft:stone_bricks" | "minecraft:mossy_stone_bricks" => {
            "minecraft:polished_blackstone_bricks"
        }
        "minecraft:cobblestone_stairs" | "minecraft:mossy_cobblestone_stairs" => {
            "minecraft:blackstone_stairs"
        }
        "minecraft:stone_stairs" => "minecraft:polished_blackstone_stairs",
        "minecraft:stone_brick_stairs" | "minecraft:mossy_stone_brick_stairs" => {
            "minecraft:polished_blackstone_brick_stairs"
        }
        "minecraft:cobblestone_slab" | "minecraft:mossy_cobblestone_slab" => {
            "minecraft:blackstone_slab"
        }
        "minecraft:smooth_stone_slab" | "minecraft:stone_slab" => {
            "minecraft:polished_blackstone_slab"
        }
        "minecraft:stone_brick_slab" | "minecraft:mossy_stone_brick_slab" => {
            "minecraft:polished_blackstone_brick_slab"
        }
        "minecraft:stone_brick_wall" | "minecraft:mossy_stone_brick_wall" => {
            "minecraft:polished_blackstone_brick_wall"
        }
        "minecraft:cobblestone_wall" | "minecraft:mossy_cobblestone_wall" => {
            "minecraft:blackstone_wall"
        }
        "minecraft:chiseled_stone_bricks" => "minecraft:chiseled_polished_blackstone",
        "minecraft:cracked_stone_bricks" => "minecraft:cracked_polished_blackstone_bricks",
        "minecraft:iron_bars" => "minecraft:iron_chain",
        _ => return processed,
    };
    let mut state = known(replacement);
    for property in ["facing", "half", "type"] {
        if let Some(value) = processed.state.property(property)
            && let Some(with) = state.with_property(property, value)
        {
            state = with;
        }
    }
    BlockInfo { state, ..processed }
}

#[cfg(test)]
mod tests {
    use clustine_noise::WorldgenRandom;

    use super::*;
    use crate::testing::World;

    fn read(data: &'static Data) -> Processor {
        Processor::read(&Reader::new(data, "test")).unwrap()
    }

    fn info(x: i32, y: i32, z: i32, state: BlockState) -> BlockInfo {
        BlockInfo {
            pos: BlockPos::new(x, y, z),
            state,
            nbt: None,
        }
    }

    /// Runs `f` with a processing context over `world`, at the origin.
    fn with_processing<R>(
        world: &mut World,
        kind: PlaceRandom,
        random: &mut WorldgenRandom,
        f: impl FnOnce(&mut Processing<'_>) -> R,
    ) -> R {
        let mut cx = Processing {
            world,
            position: BlockPos::new(0, 0, 0),
            reference: BlockPos::new(0, 0, 0),
            random_kind: kind,
            random,
            world_seed: 1234,
        };
        f(&mut cx)
    }

    static GRAVEL_TO_DIRT: Data = Data::Object(&[
        ("processor_type", Data::Text("minecraft:rule")),
        (
            "rules",
            Data::List(&[Data::Object(&[
                (
                    "input_predicate",
                    Data::Object(&[
                        ("block", Data::Text("minecraft:gravel")),
                        ("predicate_type", Data::Text("minecraft:random_block_match")),
                        ("probability", Data::Number("0.5")),
                    ]),
                ),
                (
                    "location_predicate",
                    Data::Object(&[("predicate_type", Data::Text("minecraft:always_true"))]),
                ),
                ("output_state", Data::Text("minecraft:dirt")),
            ])]),
        ),
    ]);

    #[test]
    fn a_rule_draws_from_the_blocks_place_in_the_world() {
        let processor = read(&GRAVEL_TO_DIRT);
        let mut world = World::flat(blocks::STONE, 0);
        let mut random = WorldgenRandom::from_seed(1);
        let mut turned = 0;
        for x in 0..64 {
            let original = info(x, 0, 0, blocks::GRAVEL);
            let processed = info(x, 5, 0, blocks::GRAVEL);
            let out = with_processing(&mut world, PlaceRandom::Positional, &mut random, |cx| {
                processor.process_block(cx, &original, processed)
            })
            .unwrap();
            let expected = position_random(BlockPos::new(x, 5, 0)).next_f32() < 0.5;
            assert_eq!(out.state == blocks::DIRT, expected, "at x {x}");
            turned += usize::from(expected);
        }
        assert!(turned > 10 && turned < 54, "about half turn: {turned}");
        // Another block is never drawn for, and stays.
        let stone = info(0, 5, 0, blocks::STONE);
        let out = with_processing(&mut world, PlaceRandom::Positional, &mut random, |cx| {
            processor.process_block(cx, &stone.clone(), stone.clone())
        });
        assert_eq!(out, Some(stone));
    }

    static PROTECTED: Data = Data::Object(&[
        ("processor_type", Data::Text("minecraft:protected_blocks")),
        ("value", Data::Text("#minecraft:features_cannot_replace")),
    ]);

    #[test]
    fn protected_blocks_keep_their_place() {
        let processor = read(&PROTECTED);
        let mut world = World::flat(blocks::STONE, 0);
        world.set(BlockPos::new(1, 1, 1), blocks::BEDROCK);
        let mut random = WorldgenRandom::from_seed(1);
        let on_bedrock = info(1, 1, 1, blocks::DIRT);
        let on_air = info(1, 2, 1, blocks::DIRT);
        with_processing(&mut world, PlaceRandom::Positional, &mut random, |cx| {
            assert_eq!(
                processor.process_block(cx, &on_bedrock.clone(), on_bedrock),
                None
            );
            assert!(
                processor
                    .process_block(cx, &on_air.clone(), on_air)
                    .is_some()
            );
        });
    }

    static ROT: Data = Data::Object(&[
        ("integrity", Data::Number("0.25")),
        ("processor_type", Data::Text("minecraft:block_rot")),
        ("rottable_blocks", Data::Text("minecraft:mossy_cobblestone")),
    ]);

    #[test]
    fn block_rot_draws_only_for_blocks_of_its_set_from_the_placements_source() {
        let processor = read(&ROT);
        let mut world = World::flat(blocks::STONE, 0);
        let mut random = WorldgenRandom::from_seed(7);
        let mut copy = random.clone();
        let stone = info(0, 0, 0, blocks::STONE);
        let mossy = info(0, 0, 0, blocks::MOSSY_COBBLESTONE);
        let (kept_stone, kept_mossy) =
            with_processing(&mut world, PlaceRandom::Given, &mut random, |cx| {
                (
                    processor.process_block(cx, &stone.clone(), stone.clone()),
                    processor.process_block(cx, &mossy.clone(), mossy.clone()),
                )
            });
        assert_eq!(kept_stone, Some(stone));
        assert_eq!(kept_mossy.is_some(), copy.next_f32() <= 0.25);
        // One draw was made, for the mossy cobblestone alone.
        assert_eq!(random.next_i64(), copy.next_i64());
    }

    #[test]
    fn gravity_puts_the_block_on_the_ground_at_its_height_in_the_template() {
        let processor = Processor::terrain_matching();
        let mut world = World::flat(blocks::STONE, 10);
        let mut random = WorldgenRandom::from_seed(1);
        let original = info(0, 2, 0, blocks::OAK_PLANKS);
        let processed = info(3, 40, 4, blocks::OAK_PLANKS);
        let out = with_processing(&mut world, PlaceRandom::Positional, &mut random, |cx| {
            processor.process_block(cx, &original, processed)
        });
        // The surface is at 11, less one, plus two.
        assert_eq!(out.unwrap().pos, BlockPos::new(3, 12, 4));
    }

    #[test]
    fn a_jigsaw_becomes_its_final_state_or_is_dropped_for_structure_void() {
        let jigsaw = |final_state: &str| BlockInfo {
            pos: BlockPos::new(0, 0, 0),
            state: clustine_data::blocks::JIGSAW,
            nbt: Some(Nbt::Compound(vec![(
                "final_state".into(),
                Nbt::String(final_state.into()),
            )])),
        };
        let out = jigsaw_replacement(jigsaw("minecraft:oak_stairs[facing=east]")).unwrap();
        assert_eq!(
            out.state,
            BlockState::parse("minecraft:oak_stairs[facing=east]").unwrap()
        );
        assert_eq!(out.nbt, None);
        assert_eq!(jigsaw_replacement(jigsaw("minecraft:structure_void")), None);
        assert_eq!(
            jigsaw_replacement(jigsaw("stone{x:1}")).unwrap().state,
            blocks::STONE
        );
        let plain = info(0, 0, 0, blocks::STONE);
        assert_eq!(jigsaw_replacement(plain.clone()), Some(plain));
    }

    #[test]
    fn block_ignore_drops_structure_blocks() {
        let processor = Processor::structure_block_ignore();
        let mut world = World::flat(blocks::STONE, 0);
        let mut random = WorldgenRandom::from_seed(1);
        let marker = info(0, 0, 0, blocks::STRUCTURE_BLOCK);
        let air = info(0, 0, 0, blocks::AIR);
        with_processing(&mut world, PlaceRandom::Positional, &mut random, |cx| {
            assert_eq!(processor.process_block(cx, &marker.clone(), marker), None);
            assert!(processor.process_block(cx, &air.clone(), air).is_some());
        });
    }

    #[test]
    fn lava_takes_the_place_of_a_block_that_does_not_fill_it() {
        let mut world = World::flat(blocks::STONE, 0);
        world.set(BlockPos::new(0, 5, 0), blocks::LAVA);
        let mut random = WorldgenRandom::from_seed(1);
        let slab = info(0, 5, 0, blocks::STONE_SLAB);
        let stone = info(0, 5, 0, blocks::STONE);
        let dry = info(0, 6, 0, blocks::STONE_SLAB);
        with_processing(&mut world, PlaceRandom::Positional, &mut random, |cx| {
            let p = Processor::LavaSubmergedBlock;
            assert_eq!(
                p.process_block(cx, &slab.clone(), slab).unwrap().state,
                blocks::LAVA
            );
            assert_eq!(
                p.process_block(cx, &stone.clone(), stone).unwrap().state,
                blocks::STONE
            );
            assert_eq!(
                p.process_block(cx, &dry.clone(), dry).unwrap().state,
                blocks::STONE_SLAB
            );
        });
    }

    #[test]
    fn blackstone_keeps_the_facing_and_half_of_stairs() {
        let stairs =
            BlockState::parse("minecraft:stone_brick_stairs[facing=west,half=top]").unwrap();
        let out = blackstone_replace(info(0, 0, 0, stairs));
        assert_eq!(
            out.state,
            BlockState::parse("minecraft:polished_blackstone_brick_stairs[facing=west,half=top]")
                .unwrap()
        );
        assert_eq!(
            blackstone_replace(info(0, 0, 0, blocks::IRON_BARS)).state,
            blocks::IRON_CHAIN
        );
        assert_eq!(
            blackstone_replace(info(0, 0, 0, blocks::DIRT)).state,
            blocks::DIRT
        );
    }

    #[test]
    fn block_age_draws_as_the_game_does() {
        // Obsidian: one draw, below 0.15 to cry.
        for seed in 0..32 {
            let mut random = WorldgenRandom::from_seed(seed);
            let mut copy = random.clone();
            let out = block_age(blocks::OBSIDIAN, 0.5, &mut random);
            assert_eq!(out.is_some(), copy.next_f32() < 0.15);
        }
        // Stone bricks: half the time nothing; otherwise both sets are drawn first.
        for seed in 0..32 {
            let mut random = WorldgenRandom::from_seed(seed);
            let mut copy = random.clone();
            let out = block_age(blocks::STONE_BRICKS, 0.3, &mut random);
            if copy.next_f32() >= 0.5 {
                assert_eq!(out, None);
                continue;
            }
            let _non_mossy_stairs = (copy.next_i32_bounded(4), copy.next_i32_bounded(2));
            let _mossy_stairs = (copy.next_i32_bounded(4), copy.next_i32_bounded(2));
            let mossy = copy.next_f32() < 0.3;
            let which = copy.next_i32_bounded(2);
            let name = out.unwrap().block().unwrap().name;
            let expected = match (mossy, which) {
                (true, 0) => "minecraft:mossy_stone_bricks",
                (true, _) => "minecraft:mossy_stone_brick_stairs",
                (false, 0) => "minecraft:cracked_stone_bricks",
                (false, _) => "minecraft:stone_brick_stairs",
            };
            assert_eq!(name, expected);
            assert_eq!(random.next_i64(), copy.next_i64());
        }
    }

    static CAPPED: Data = Data::Object(&[
        (
            "delegate",
            Data::Object(&[
                ("processor_type", Data::Text("minecraft:rule")),
                (
                    "rules",
                    Data::List(&[Data::Object(&[
                        (
                            "block_entity_modifier",
                            Data::Object(&[
                                ("type", Data::Text("minecraft:append_loot")),
                                (
                                    "loot_table",
                                    Data::Text("minecraft:archaeology/trail_ruins_common"),
                                ),
                            ]),
                        ),
                        (
                            "input_predicate",
                            Data::Object(&[
                                ("predicate_type", Data::Text("minecraft:block_match")),
                                ("block", Data::Text("minecraft:gravel")),
                            ]),
                        ),
                        (
                            "location_predicate",
                            Data::Object(&[(
                                "predicate_type",
                                Data::Text("minecraft:always_true"),
                            )]),
                        ),
                        ("output_state", Data::Text("minecraft:suspicious_gravel")),
                    ])]),
                ),
            ]),
        ),
        ("limit", Data::Number("3")),
        ("processor_type", Data::Text("minecraft:capped")),
    ]);

    #[test]
    fn capped_changes_at_most_its_limit_chosen_by_the_worlds_seed_at_the_position() {
        let processor = read(&CAPPED);
        let mut world = World::flat(blocks::STONE, 0);
        let mut random = WorldgenRandom::from_seed(1);
        let originals: Vec<BlockInfo> = (0..10)
            .map(|x| {
                info(
                    x,
                    0,
                    0,
                    if x % 2 == 0 {
                        blocks::GRAVEL
                    } else {
                        blocks::DIRT
                    },
                )
            })
            .collect();
        let processed: Vec<BlockInfo> = originals
            .iter()
            .map(|block| info(block.pos.x, 20, 0, block.state))
            .collect();
        // Every block passes through `processBlock` unchanged.
        let out = with_processing(&mut world, PlaceRandom::Positional, &mut random, |cx| {
            let passed: Vec<BlockInfo> = originals
                .iter()
                .zip(processed.iter())
                .map(|(o, p)| processor.process_block(cx, o, p.clone()).unwrap())
                .collect();
            assert_eq!(passed, processed);
            processor.finalize(cx, &originals, passed)
        });
        let changed: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, block)| block.state != blocks::GRAVEL && block.state != blocks::DIRT)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(changed.len(), 3);

        // The same choice, worked out from the game's description.
        let mut shuffle = LegacyRandom::from_seed(1234_u64)
            .next_positional()
            .at(0, 0, 0);
        assert_eq!(IntProvider::Constant(3).sample(&mut shuffle), 3);
        let mut indices: Vec<usize> = (0..10).collect();
        for i in (2..=10).rev() {
            let j = shuffle.next_i32_bounded(i as i32) as usize;
            indices.swap(i - 1, j);
        }
        let mut expected: Vec<usize> = indices.into_iter().filter(|i| i % 2 == 0).take(3).collect();
        expected.sort_unstable();
        assert_eq!(changed, expected);
        for index in changed {
            let nbt = out[index].nbt.as_ref().unwrap();
            assert_eq!(
                nbt.get("LootTable"),
                Some(&Nbt::String(
                    "minecraft:archaeology/trail_ruins_common".into()
                ))
            );
            assert!(matches!(nbt.get("LootTableSeed"), Some(Nbt::Long(_))));
        }
    }

    #[test]
    fn a_linear_position_test_falls_from_one_chance_to_the_other() {
        let test = PosRuleTest::AxisAlignedLinearPos {
            axis: Axis::Y,
            min_chance: 0.0,
            max_chance: 0.05,
            min_dist: 0,
            max_dist: 100,
        };
        let reference = BlockPos::new(0, 0, 0);
        for y in [0, 50, 100, 200] {
            let mut random = LegacyRandom::from_seed(5);
            let mut copy = random.clone();
            let chance = (y.min(100) as f32 / 100.0) * 0.05;
            assert_eq!(
                test.test(BlockPos::new(9, y, 9), reference, &mut random),
                copy.next_f32() <= chance
            );
        }
        // Equal ends: at the end itself no draw passes; beyond it the far chance holds.
        let flat = PosRuleTest::LinearPos {
            min_chance: 1.0,
            max_chance: 1.0,
            min_dist: 3,
            max_dist: 3,
        };
        let mut random = LegacyRandom::from_seed(5);
        assert!(!flat.test(BlockPos::new(3, 0, 0), reference, &mut random));
        assert!(flat.test(BlockPos::new(4, 0, 0), reference, &mut random));
    }

    #[test]
    fn modifiers_clear_merge_and_append_loot() {
        let mut random = LegacyRandom::from_seed(3);
        let existing = Some(Nbt::Compound(vec![(
            "a".into(),
            Nbt::Compound(vec![("x".into(), Nbt::Int(1))]),
        )]));
        assert_eq!(
            BlockEntityModifier::Clear.apply(&mut random, existing.clone()),
            Some(Nbt::Compound(Vec::new()))
        );
        assert_eq!(
            BlockEntityModifier::Passthrough.apply(&mut random, None),
            None
        );
        let merged = BlockEntityModifier::AppendStatic(Nbt::Compound(vec![
            ("a".into(), Nbt::Compound(vec![("y".into(), Nbt::Int(2))])),
            ("b".into(), Nbt::Byte(1)),
        ]))
        .apply(&mut random, existing)
        .unwrap();
        assert!(nbt_equal(
            &merged,
            &Nbt::Compound(vec![
                (
                    "a".into(),
                    Nbt::Compound(vec![("x".into(), Nbt::Int(1)), ("y".into(), Nbt::Int(2))])
                ),
                ("b".into(), Nbt::Byte(1)),
            ])
        ));
        let mut copy = random.clone();
        let loot = BlockEntityModifier::AppendLoot("minecraft:chests/x".into())
            .apply(&mut random, None)
            .unwrap();
        assert_eq!(loot.get("LootTableSeed"), Some(&Nbt::Long(copy.next_i64())));
    }

    static LIST: Data = Data::Object(&[(
        "processors",
        Data::List(&[
            Data::Object(&[("processor_type", Data::Text("minecraft:nop"))]),
            Data::Object(&[
                ("processor_type", Data::Text("minecraft:gravity")),
                ("heightmap", Data::Text("OCEAN_FLOOR_WG")),
                ("offset", Data::Number("2")),
            ]),
            Data::Object(&[
                ("processor_type", Data::Text("minecraft:block_ignore")),
                ("blocks", Data::List(&[Data::Text("minecraft:air")])),
            ]),
        ]),
    )]);

    static NAMED: Data = Data::Text("minecraft:fossil_rot");

    static UNKNOWN_MEMBER: Data = Data::Object(&[
        ("processor_type", Data::Text("minecraft:nop")),
        ("surprise", Data::Bool(true)),
    ]);

    #[test]
    fn a_list_is_read_in_place_or_by_name_and_an_unknown_member_fails() {
        let none = |_: &str| None;
        let list = ProcessorList::read(&Reader::new(&LIST, "test"), &none).unwrap();
        assert_eq!(
            list.processors,
            [
                Processor::Nop,
                Processor::Gravity {
                    heightmap: Heightmap::OceanFloorWg,
                    offset: 2
                },
                Processor::air_ignore(),
            ]
        );
        let by_name = |name: &str| (name == "minecraft:fossil_rot").then_some(&LIST);
        assert_eq!(
            ProcessorList::read(&Reader::new(&NAMED, "test"), &by_name).unwrap(),
            list
        );
        assert!(ProcessorList::read(&Reader::new(&NAMED, "test"), &none).is_err());
        assert!(Processor::read(&Reader::new(&UNKNOWN_MEMBER, "test")).is_err());
    }

    #[test]
    fn json_numbers_become_the_narrowest_tag() {
        assert_eq!(data_to_nbt(&Data::Number("3")), Nbt::Byte(3));
        assert_eq!(data_to_nbt(&Data::Number("300")), Nbt::Short(300));
        assert_eq!(data_to_nbt(&Data::Number("70000")), Nbt::Int(70000));
        assert_eq!(data_to_nbt(&Data::Number("0.5")), Nbt::Float(0.5));
        assert_eq!(data_to_nbt(&Data::Number("0.1")), Nbt::Double(0.1));
        assert_eq!(data_to_nbt(&Data::Bool(true)), Nbt::Byte(1));
    }
}
