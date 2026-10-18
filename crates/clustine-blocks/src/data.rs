//! What the rules ask of the game's data: classes, tags, fluids and properties, and
//! the directions as the game's `Direction` offers them.

use std::sync::LazyLock;

use clustine_data::{BlockClass, BlockState, Face, TAGS};
use clustine_world::BlockPos;

/// The horizontal directions in the game's order (`Direction.Plane.HORIZONTAL`).
pub const HORIZONTAL: [Face; 4] = [Face::North, Face::East, Face::South, Face::West];

/// The members of the tag `name` of `registry`, as ids in that registry.
fn tag(registry: &str, name: &str) -> Option<&'static [i32]> {
    TAGS.iter()
        .find(|tags| tags.registry == registry)
        .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
        .map(|tag| tag.entries)
}

/// Whether the block of `state` is in the block tag `name`, such as
/// `minecraft:walls`. A tag the game does not have holds nothing; a test asks every
/// tag the rules name.
pub fn in_tag(state: BlockState, name: &str) -> bool {
    let Some(id) = state.block_id() else {
        return false;
    };
    tag("minecraft:block", name).is_some_and(|entries| entries.contains(&(id as i32)))
}

/// Whether the item `item` (its id) is in the item tag `name`, such as
/// `minecraft:noteblock_top_instruments`.
pub fn item_in_tag(item: i32, name: &str) -> bool {
    tag("minecraft:item", name).is_some_and(|entries| entries.contains(&item))
}

/// A block tag the rules ask often, looked up once.
pub struct Tag {
    name: &'static str,
    entries: LazyLock<&'static [i32]>,
}

impl Tag {
    const fn new(name: &'static str, entries: fn() -> &'static [i32]) -> Self {
        Self {
            name,
            entries: LazyLock::new(entries),
        }
    }

    pub fn contains(&self, state: BlockState) -> bool {
        let Some(id) = state.block_id() else {
            return false;
        };
        self.entries.contains(&(id as i32))
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

macro_rules! tag {
    ($name:ident, $tag:literal) => {
        pub static $name: Tag = Tag::new($tag, || {
            tag("minecraft:block", $tag).expect(concat!("the game has the tag ", $tag))
        });
    };
}

tag!(
    PREVENTS_NEARBY_LEAF_DECAY,
    "minecraft:prevents_nearby_leaf_decay"
);
tag!(JUNGLE_LOGS, "minecraft:jungle_logs");
tag!(SNOW, "minecraft:snow");
tag!(MAINTAINS_FARMLAND, "minecraft:maintains_farmland");
tag!(BLOCKS_FLUID_FLOW, "minecraft:blocks_fluid_flow");
tag!(FENCES, "minecraft:fences");
tag!(WALLS, "minecraft:walls");
tag!(FALL_DAMAGE_RESETTING, "minecraft:fall_damage_resetting");
tag!(SUPPRESSES_BOUNCE, "minecraft:suppresses_bounce");

/// The fluid in `state` by the game's ids (`clustine_data::FLUIDS`): empty (0) where
/// there is none.
pub fn fluid_id(state: BlockState) -> u8 {
    state.props().fluid().map_or(0, |fluid| fluid.fluid)
}

/// Whether the fluid in `state` is in the fluid tag `name`, such as `minecraft:water`.
pub fn fluid_in_tag(state: BlockState, name: &str) -> bool {
    let Some(fluid) = state.props().fluid() else {
        return false;
    };
    tag("minecraft:fluid", name).is_some_and(|entries| entries.contains(&i32::from(fluid.fluid)))
}

/// Whether `state` holds water of any kind, the game's `is(FluidTags.WATER)`.
pub fn has_water(state: BlockState) -> bool {
    fluid_in_tag(state, "minecraft:water")
}

/// Whether `state` holds lava of any kind.
pub fn has_lava(state: BlockState) -> bool {
    fluid_in_tag(state, "minecraft:lava")
}

/// Whether the block of `state` is of `class` or of a class made from it.
pub fn is_class(state: BlockState, class: BlockClass) -> bool {
    state.block_info().is_some_and(|info| info.is(class))
}

/// Whether two states are of the same block.
pub fn same_block(a: BlockState, b: BlockState) -> bool {
    a.block_id() == b.block_id()
}

/// Whether `state` is a block's state and of the block of `block` (`state.is(block)`).
pub fn is_block(state: BlockState, block: BlockState) -> bool {
    state.block_id().is_some() && state.block_id() == block.block_id()
}

/// The id of the block of `state`, as a tick of it names it.
pub fn tick_id(state: BlockState) -> u16 {
    state
        .block_id()
        .expect("a block that schedules a tick of itself is a block's state") as u16
}

/// Whether the boolean property `name` of `state` is true.
pub fn flag(state: BlockState, name: &str) -> bool {
    state.property(name) == Some("true")
}

/// The value of the property `name` of `state` as a number.
pub fn number(state: BlockState, name: &str) -> Option<u8> {
    state.property(name)?.parse().ok()
}

/// `state` with the property `name` set to `value`, or `state` itself if its block has
/// no such property: rules set what a class has, and some blocks of a class lack one.
pub fn set(state: BlockState, name: &str, value: &str) -> BlockState {
    state.with_property(name, value).unwrap_or(state)
}

/// `state` with the boolean property `name` set.
pub fn set_flag(state: BlockState, name: &str, value: bool) -> BlockState {
    set(state, name, if value { "true" } else { "false" })
}

/// The name the game gives a direction, which is also the name of a block's property
/// for that side.
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

/// The direction called `name`.
pub fn face_named(name: &str) -> Option<Face> {
    Face::ALL.into_iter().find(|face| face_name(*face) == name)
}

/// The value of a `facing` property.
pub fn facing(state: BlockState) -> Option<Face> {
    state.property("facing").and_then(face_named)
}

/// The block next to `pos` towards `face` (`relative`).
pub fn relative(pos: BlockPos, face: Face) -> BlockPos {
    let (x, y, z) = match face {
        Face::Down => (0, -1, 0),
        Face::Up => (0, 1, 0),
        Face::North => (0, 0, -1),
        Face::South => (0, 0, 1),
        Face::West => (-1, 0, 0),
        Face::East => (1, 0, 0),
    };
    pos.offset(x, y, z)
}

/// An axis of the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// The axis a direction lies along.
pub fn axis(face: Face) -> Axis {
    match face {
        Face::Down | Face::Up => Axis::Y,
        Face::North | Face::South => Axis::Z,
        Face::West | Face::East => Axis::X,
    }
}

/// Whether a direction lies in the horizontal plane.
pub fn is_horizontal(face: Face) -> bool {
    axis(face) != Axis::Y
}

/// Turned a quarter clockwise about the vertical, as seen from above.
pub fn clockwise(face: Face) -> Face {
    match face {
        Face::North => Face::East,
        Face::East => Face::South,
        Face::South => Face::West,
        Face::West => Face::North,
        vertical => vertical,
    }
}

/// Turned a quarter counter-clockwise about the vertical, as seen from above.
pub fn counter_clockwise(face: Face) -> Face {
    match face {
        Face::North => Face::West,
        Face::West => Face::South,
        Face::South => Face::East,
        Face::East => Face::North,
        vertical => vertical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tag_the_rules_ask_is_in_the_games_data() {
        // A tag the data lacks would hold nothing and quietly change a rule's answer.
        for name in [
            "blocks_fluid_flow",
            "fall_damage_resetting",
            "suppresses_bounce",
            "walls",
            "cannot_support_seagrass",
            "cannot_support_snow_layer",
            "fences",
            "jungle_logs",
            "maintains_farmland",
            "overrides_mushroom_light_requirement",
            "prevents_nearby_leaf_decay",
            "shulker_boxes",
            "snow",
            "soul_fire_base_blocks",
            "support_override_cactus_flower",
            "support_override_snow_layer",
            "supports_azalea",
            "supports_bamboo",
            "supports_big_dripleaf",
            "supports_cactus",
            "supports_crimson_fungus",
            "supports_crimson_roots",
            "supports_crops",
            "supports_dry_vegetation",
            "supports_frogspawn",
            "supports_hanging_mangrove_propagule",
            "supports_lily_pad",
            "supports_mangrove_propagule",
            "supports_melon_stem",
            "supports_nether_sprouts",
            "supports_nether_wart",
            "supports_pumpkin_stem",
            "supports_small_dripleaf",
            "supports_sugar_cane",
            "supports_sugar_cane_adjacently",
            "supports_vegetation",
            "supports_warped_fungus",
            "supports_warped_roots",
            "supports_wither_rose",
            "wall_post_override",
            "walls",
            "wooden_fences",
        ] {
            let name = format!("minecraft:{name}");
            assert!(tag("minecraft:block", &name).is_some(), "{name}");
        }
        for name in [
            "water",
            "lava",
            "supports_lily_pad",
            "supports_frogspawn",
            "supports_sugar_cane_adjacently",
        ] {
            let name = format!("minecraft:{name}");
            assert!(tag("minecraft:fluid", &name).is_some(), "{name}");
        }
    }

    #[test]
    fn a_direction_turns_and_is_named_as_the_game_names_it() {
        for face in Face::ALL {
            assert_eq!(face_named(face_name(face)), Some(face));
        }
        for face in HORIZONTAL {
            assert_eq!(counter_clockwise(clockwise(face)), face);
            assert_ne!(axis(clockwise(face)), axis(face));
        }
        assert_eq!(clockwise(Face::North), Face::East);
    }
}
