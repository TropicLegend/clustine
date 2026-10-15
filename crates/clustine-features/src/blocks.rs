//! What features read of blocks in the data and in the world: a block state as the
//! data writes one, sets of blocks and fluids (the game's `HolderSet`), the game's tags
//! and directions.

use clustine_data::{BLOCKS, BlockClass, BlockState, FLUIDS, Face, TAGS, block_id_by_name};
use clustine_world::BlockPos;

use crate::reader::{Read, Reader};

/// Reads a block state as 26.3's codec reads one: a block's name for its default
/// state, or `{"id": ..., "properties": {...}}` with the properties given and the
/// rest as in the default state.
pub fn read_state(reader: &Reader) -> Read<BlockState> {
    if reader.is_text() {
        let name = reader.text()?;
        return block_default(name).ok_or_else(|| reader.error(format!("{name} is not a block")));
    }
    let name = reader.field("id")?.text()?;
    let mut pairs = Vec::new();
    if let Some(properties) = reader.optional("properties") {
        let clustine_worldgen_data::feature::Data::Object(members) = properties.data() else {
            return Err(properties.error("the properties of a block state are an object"));
        };
        for (property, value) in members.iter() {
            let clustine_worldgen_data::feature::Data::Text(value) = value else {
                return Err(properties.error(format!("{property} is not a string")));
            };
            pairs.push((*property, *value));
        }
    }
    reader.finish()?;
    BlockState::from_name_and_properties(name, pairs.iter().copied())
        .ok_or_else(|| reader.error(format!("{name} with {pairs:?} is not a block state")))
}

/// The default state of the block called `name`.
pub fn block_default(name: &str) -> Option<BlockState> {
    Some(BLOCKS[block_id_by_name(name)?].default_state)
}

/// The id of the block of `state`. Every state a feature reads is a block's.
pub fn block_id(state: BlockState) -> usize {
    state
        .block_id()
        .expect("every state a feature reads is a block's state")
}

/// Whether two states are of the same block (`state.is(block)`).
pub fn same_block(a: BlockState, b: BlockState) -> bool {
    a.block_id() == b.block_id()
}

/// Whether the block of `state` is of `class` or of a class made from it.
pub fn is_class(state: BlockState, class: BlockClass) -> bool {
    state.block_info().is_some_and(|info| info.is(class))
}

/// The members of the tag `name` (with its namespace, without `#`) of `registry`.
fn tag(registry: &str, name: &str) -> Option<&'static [i32]> {
    TAGS.iter()
        .find(|tags| tags.registry == registry)
        .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
        .map(|tag| tag.entries)
}

/// The blocks of the block tag `name`, in the game's order.
pub fn block_tag(name: &str) -> Option<&'static [i32]> {
    tag("minecraft:block", name)
}

/// Whether the block of `state` is in the block tag `name`. A tag the game does not
/// have holds nothing.
pub fn in_block_tag(state: BlockState, name: &str) -> bool {
    let id = block_id(state) as i32;
    block_tag(name).is_some_and(|entries| entries.contains(&id))
}

/// The fluid in `state` by the game's ids ([`FLUIDS`]): empty (0) where there is none.
pub fn fluid_id(state: BlockState) -> u8 {
    state.props().fluid().map_or(0, |fluid| fluid.fluid)
}

/// Whether the fluid in `state` is in the fluid tag `name` (`minecraft:water`).
pub fn fluid_in_tag(state: BlockState, name: &str) -> bool {
    let id = i32::from(fluid_id(state));
    tag("minecraft:fluid", name).is_some_and(|entries| entries.contains(&id))
}

/// `level.isWaterAt`: the fluid in `state` is water, flowing or not.
pub fn has_water(state: BlockState) -> bool {
    fluid_in_tag(state, "minecraft:water")
}

/// A set of blocks as the data names one (`HolderSet<Block>`): a block, a tag with
/// `#`, or a list of blocks. The blocks are kept in the game's order, which a random
/// pick from the set follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSet {
    blocks: Vec<usize>,
}

impl BlockSet {
    pub fn read(reader: &Reader) -> Read<Self> {
        let names: Vec<&str> = if reader.is_list() {
            reader
                .list()?
                .iter()
                .map(Reader::text)
                .collect::<Read<_>>()?
        } else {
            vec![reader.text()?]
        };
        let mut blocks = Vec::new();
        for name in names {
            if let Some(tag) = name.strip_prefix('#') {
                let entries =
                    block_tag(tag).ok_or_else(|| reader.error(format!("{tag} is not a tag")))?;
                blocks.extend(entries.iter().map(|id| *id as usize));
            } else {
                blocks.push(
                    block_id_by_name(name)
                        .ok_or_else(|| reader.error(format!("{name} is not a block")))?,
                );
            }
        }
        Ok(Self { blocks })
    }

    /// The set of these blocks, by name; for tests.
    #[cfg(test)]
    pub fn of(names: &[&str]) -> Self {
        Self {
            blocks: names
                .iter()
                .map(|name| block_id_by_name(name).expect("a block"))
                .collect(),
        }
    }

    pub fn contains(&self, state: BlockState) -> bool {
        self.blocks.contains(&block_id(state))
    }

    /// The blocks' ids, in order.
    pub fn blocks(&self) -> &[usize] {
        &self.blocks
    }
}

/// A set of fluids as the data names one (`HolderSet<Fluid>`), by the game's ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluidSet {
    fluids: Vec<u8>,
}

impl FluidSet {
    pub fn read(reader: &Reader) -> Read<Self> {
        let names: Vec<&str> = if reader.is_list() {
            reader
                .list()?
                .iter()
                .map(Reader::text)
                .collect::<Read<_>>()?
        } else {
            vec![reader.text()?]
        };
        let mut fluids = Vec::new();
        for name in names {
            if let Some(name) = name.strip_prefix('#') {
                let entries = tag("minecraft:fluid", name)
                    .ok_or_else(|| reader.error(format!("{name} is not a fluid tag")))?;
                fluids.extend(entries.iter().map(|id| *id as u8));
            } else {
                let id = FLUIDS
                    .iter()
                    .position(|fluid| *fluid == name)
                    .ok_or_else(|| reader.error(format!("{name} is not a fluid")))?;
                fluids.push(id as u8);
            }
        }
        Ok(Self { fluids })
    }

    /// Whether the fluid in `state` is one of the set (`getFluidState().is(set)`).
    pub fn contains(&self, state: BlockState) -> bool {
        self.fluids.contains(&fluid_id(state))
    }
}

/// The direction a data file names (`up`, `north`).
pub fn face_named(name: &str) -> Option<Face> {
    Some(match name {
        "down" => Face::Down,
        "up" => Face::Up,
        "north" => Face::North,
        "south" => Face::South,
        "west" => Face::West,
        "east" => Face::East,
        _ => return None,
    })
}

/// Reads a direction.
pub fn read_face(reader: &Reader) -> Read<Face> {
    let name = reader.text()?;
    face_named(name).ok_or_else(|| reader.error(format!("{name} is not a direction")))
}

/// The name the game gives a direction.
pub fn face_name(face: Face) -> &'static str {
    match face {
        Face::Down => "down",
        Face::Up => "up",
        Face::North => "north",
        Face::South => "south",
        Face::West => "west",
        Face::East => "east",
    }
}

/// The step one block towards `face`.
pub fn step(face: Face) -> (i32, i32, i32) {
    match face {
        Face::Down => (0, -1, 0),
        Face::Up => (0, 1, 0),
        Face::North => (0, 0, -1),
        Face::South => (0, 0, 1),
        Face::West => (-1, 0, 0),
        Face::East => (1, 0, 0),
    }
}

/// The block next to `pos` towards `face` (`relative`).
pub fn relative(pos: BlockPos, face: Face) -> BlockPos {
    let (x, y, z) = step(face);
    BlockPos::new(pos.x + x, pos.y + y, pos.z + z)
}

/// The block `distance` blocks from `pos` towards `face`.
pub fn relative_by(pos: BlockPos, face: Face, distance: i32) -> BlockPos {
    let (x, y, z) = step(face);
    BlockPos::new(
        pos.x + x * distance,
        pos.y + y * distance,
        pos.z + z * distance,
    )
}

/// The horizontal directions in the game's order (`Direction.Plane.HORIZONTAL`).
pub const HORIZONTAL: [Face; 4] = [Face::North, Face::East, Face::South, Face::West];

/// Whether the direction is horizontal.
pub fn is_horizontal(face: Face) -> bool {
    !matches!(face, Face::Up | Face::Down)
}

/// The axis of a direction as the property `axis` names it.
pub fn axis_name(face: Face) -> &'static str {
    match face {
        Face::Down | Face::Up => "y",
        Face::North | Face::South => "z",
        Face::West | Face::East => "x",
    }
}

/// Reads an offset `[x, y, z]`, or none where the member is missing.
pub fn read_offset(reader: &Reader, key: &str) -> Read<(i32, i32, i32)> {
    match reader.optional(key) {
        Some(offset) => read_vec(&offset),
        None => Ok((0, 0, 0)),
    }
}

/// Reads three integers written as a list (`Vec3i`, `BlockPos`).
pub fn read_vec(reader: &Reader) -> Read<(i32, i32, i32)> {
    let items = reader.list()?;
    let [x, y, z] = items.as_slice() else {
        return Err(reader.error("is not three numbers"));
    };
    Ok((x.int()?, y.int()?, z.int()?))
}

/// `pos` moved by `offset`.
pub fn offset(pos: BlockPos, (x, y, z): (i32, i32, i32)) -> BlockPos {
    BlockPos::new(pos.x + x, pos.y + y, pos.z + z)
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature::Data;

    use super::*;

    #[test]
    fn a_state_is_read_from_a_name_or_an_id_with_properties() {
        static NAME: Data = Data::Text("minecraft:stone");
        static FULL: Data = Data::Object(&[
            ("id", Data::Text("minecraft:basalt")),
            ("properties", Data::Object(&[("axis", Data::Text("x"))])),
        ]);
        assert_eq!(read_state(&Reader::new(&NAME, "t")), Ok(blocks::STONE));
        let basalt = read_state(&Reader::new(&FULL, "t")).unwrap();
        assert_eq!(basalt.property("axis"), Some("x"));
        assert!(same_block(basalt, blocks::BASALT));
    }

    #[test]
    fn a_set_of_blocks_is_a_block_a_tag_or_a_list() {
        static TAG: Data = Data::Text("#minecraft:base_stone_overworld");
        static LIST: Data =
            Data::List(&[Data::Text("minecraft:dirt"), Data::Text("minecraft:sand")]);
        let stones = BlockSet::read(&Reader::new(&TAG, "t")).unwrap();
        assert!(stones.contains(blocks::DEEPSLATE));
        assert!(!stones.contains(blocks::DIRT));
        let list = BlockSet::read(&Reader::new(&LIST, "t")).unwrap();
        assert!(list.contains(blocks::SAND) && list.contains(blocks::DIRT));
        assert!(!list.contains(blocks::STONE));
    }

    #[test]
    fn a_set_of_fluids_tells_still_water_from_flowing() {
        static WATER: Data = Data::Text("minecraft:water");
        let water = FluidSet::read(&Reader::new(&WATER, "t")).unwrap();
        assert!(water.contains(blocks::WATER));
        assert!(!water.contains(blocks::WATER.with_property("level", "3").unwrap()));
        assert!(!water.contains(blocks::AIR));
    }
}
