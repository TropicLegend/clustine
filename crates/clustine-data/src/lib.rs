//! Game data generated from the official server jar: registries, block states and what
//! the game's code says of them, tags.
//!
//! Everything under `generated` is written by `cargo datagen` and must not be edited by
//! hand; see `docs/adr/0004-game-data.md` and
//! `docs/adr/0019-data-made-from-mojangs-jar.md`. All ids are those of
//! [`GAME_VERSION`]. The data in the generated files is Mojang's and not under the
//! licence of the code around it; see `NOTICE.md` at the root of the repository.

#[rustfmt::skip]
mod generated;

pub mod biome_parameters;
pub mod block_states;
pub mod faces;
pub mod packed;

use serde::{Deserialize, Serialize};

pub use biome_parameters::{BiomeParameterList, BiomeParameters};
pub use block_states::{Face, FaceShape, FluidProps, PushReaction, Shape, StateProps, SupportType};
pub use generated::block_classes::{BLOCK_ENTITY_TYPES, BLOCK_INFO, BlockClass, FLUIDS};
pub use generated::blocks::{BLOCK_STATE_COUNT, BLOCKS};
pub use generated::dimension_types::DIMENSION_TYPES;
pub use generated::entity_types::ENTITY_TYPES;
pub use generated::items::{ITEMS, ItemClass};
pub use generated::registries::SYNCED_REGISTRIES;
pub use generated::sound_events::SOUND_EVENTS;
pub use generated::tags::TAGS;
pub use generated::version::{
    DATA_VERSION, GAME_VERSION, PROTOCOL_VERSION, SERVER_JAR_SHA1, SERVER_JAR_SHA1_BYTES,
    SERVER_JAR_URL,
};

use generated::block_properties::{
    BLOCK_PROPERTY_LISTS, BLOCKS_BY_NAME, PROPERTIES, PROPERTY_LISTS,
};

/// The default state of every block, by name.
pub mod blocks {
    pub use crate::generated::blocks::default_states::*;
}

/// The id of every item, by name.
pub mod items {
    pub use crate::generated::items::ids::*;
}

/// The id of every entity type, by name.
pub mod entity_types {
    pub use crate::generated::entity_types::ids::*;
}

/// The ids of the sound events, named as the game's `SoundEvents` names them.
pub mod sounds {
    pub use crate::generated::sound_events::ids::*;
}

/// One concrete combination of a block and its property values, identified by its
/// numeric id.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct BlockState(pub u16);

impl BlockState {
    /// The block this state belongs to, or `None` if the id is not a block state.
    pub fn block(self) -> Option<&'static Block> {
        BLOCKS.get(self.block_id()?)
    }

    /// The id of the block this state belongs to, which is its index in [`BLOCKS`] and
    /// [`BLOCK_INFO`], or `None` if the id is not a block state.
    pub fn block_id(self) -> Option<usize> {
        let index = BLOCKS.partition_point(|block| block.last_state < self);
        (index < BLOCKS.len()).then_some(index)
    }

    /// What the game's code says of the block this state belongs to: its class, its
    /// block entity type, whether a click uses it. `None` if the id is not a block
    /// state.
    pub fn block_info(self) -> Option<&'static BlockInfo> {
        BLOCK_INFO.get(self.block_id()?)
    }

    /// The state of the block called `name` whose properties have the values given;
    /// a property that is not given has the value it has in the block's default state,
    /// as in the game. `None` if there is no such block, property or value.
    ///
    /// This is how a block state written in the game's data becomes an id, whichever
    /// way it is written there: a bare name, a name with some properties, or all of
    /// them.
    pub fn from_name_and_properties<'a>(
        name: &str,
        properties: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Option<BlockState> {
        let block = &BLOCKS[block_id_by_name(name)?];
        let mut state = block.default_state;
        for (property, value) in properties {
            state = state.with_property(property, value)?;
        }
        Some(state)
    }

    /// The state that `text` names in the form the game prints one in, such as
    /// `minecraft:oak_stairs[facing=east,half=top]` or `minecraft:stone`. Properties
    /// that are left out have their default values.
    pub fn parse(text: &str) -> Option<BlockState> {
        let Some((name, rest)) = text.split_once('[') else {
            return Self::from_name_and_properties(text, []);
        };
        let inner = rest.strip_suffix(']')?;
        let mut pairs = Vec::new();
        for pair in inner.split(',').filter(|pair| !pair.is_empty()) {
            pairs.push(pair.split_once('=')?);
        }
        Self::from_name_and_properties(name, pairs)
    }

    /// The properties of this state with their values, in the order the game numbers
    /// states by. Empty if the id is not a block state.
    pub fn properties(self) -> impl Iterator<Item = (&'static str, &'static str)> {
        let block_id = self.block_id();
        let mut index = block_id.map_or(0, |id| usize::from(self.0 - BLOCKS[id].first_state.0));
        let properties: &'static [u16] = block_id.map_or(&[], property_list);
        // The first property counts most, so the values come out last to first.
        let mut values = vec![""; properties.len()];
        for (slot, &property) in values.iter_mut().zip(properties).rev() {
            let property = &PROPERTIES[usize::from(property)];
            *slot = property.values[index % property.values.len()];
            index /= property.values.len();
        }
        properties
            .iter()
            .map(|&property| PROPERTIES[usize::from(property)].name)
            .zip(values)
    }

    /// The value of the property called `name`, or `None` if the block has none of that
    /// name.
    pub fn property(self, name: &str) -> Option<&'static str> {
        self.properties()
            .find(|(property, _)| *property == name)
            .map(|(_, value)| value)
    }

    /// This state with the property called `name` set to `value`, or `None` if the
    /// block has no such property or the property no such value.
    pub fn with_property(self, name: &str, value: &str) -> Option<BlockState> {
        let block = &BLOCKS[self.block_id()?];
        let properties = property_list(self.block_id()?);
        let index = usize::from(self.0 - block.first_state.0);
        // A property's stride is the product of the sizes of those after it.
        let mut stride = 1;
        for &property in properties.iter().rev() {
            let property = &PROPERTIES[usize::from(property)];
            let size = property.values.len();
            if property.name == name {
                let wanted = property.values.iter().position(|&known| known == value)?;
                let current = index / stride % size;
                let changed = index - current * stride + wanted * stride;
                return Some(BlockState(block.first_state.0 + changed as u16));
            }
            stride *= size;
        }
        None
    }
}

/// A block and its contiguous range of states. Indexed by block id in [`BLOCKS`].
#[derive(Debug)]
pub struct Block {
    pub name: &'static str,
    pub first_state: BlockState,
    pub last_state: BlockState,
    pub default_state: BlockState,
}

/// The id of the block called `name`, such as `minecraft:stone`; its index in
/// [`BLOCKS`] and [`BLOCK_INFO`].
pub fn block_id_by_name(name: &str) -> Option<usize> {
    let found = BLOCKS_BY_NAME
        .binary_search_by(|&id| BLOCKS[usize::from(id)].name.cmp(name))
        .ok()?;
    Some(usize::from(BLOCKS_BY_NAME[found]))
}

/// The properties of the block with this id, in the order the game numbers its states
/// by: the first counts most. Panics if there is no such block.
pub fn block_properties(block_id: usize) -> impl Iterator<Item = &'static BlockProperty> {
    property_list(block_id)
        .iter()
        .map(|&property| &PROPERTIES[usize::from(property)])
}

fn property_list(block_id: usize) -> &'static [u16] {
    PROPERTY_LISTS[usize::from(BLOCK_PROPERTY_LISTS[block_id])]
}

/// A property of a block with the values it can have, in the game's order.
#[derive(Debug)]
pub struct BlockProperty {
    pub name: &'static str,
    pub values: &'static [&'static str],
}

/// What the game's code says of a block and its data files do not. Indexed by block id
/// in [`BLOCK_INFO`].
#[derive(Debug)]
pub struct BlockInfo {
    /// The block's class in the game. Behaviours that Clustine ports (whether a plant
    /// can stay where it is, how a block turns with a structure, what it becomes when
    /// a neighbour changes) are chosen by it.
    pub class: BlockClass,
    /// The id of the block's block entity type, an index into [`BLOCK_ENTITY_TYPES`],
    /// if the block keeps a block entity.
    pub block_entity_type: Option<u8>,
    /// Whether a click on the block with the use button is taken by the block, so
    /// that a block in the hand is not placed against it. It holds where the block's
    /// class handles a click without an item and, for a door or a trapdoor, where its
    /// material opens by hand: an oak door is used, an iron door is not. A block whose
    /// class answers by its state or by what is held, such as a cake, counts as used.
    pub used_by_click: bool,
    /// Whether the block's class, or one it is made from, says where the block can stay
    /// (the game's `canSurvive`): what it needs below it or behind it.
    pub survival_rule: bool,
    /// For a flower pot, the id of the block it holds: air for the empty pot.
    pub potted: Option<u16>,
}

impl BlockInfo {
    /// Whether the block's class is `class` or is made from it: a copper door is a
    /// door, a repeater a diode.
    pub fn is(&self, class: BlockClass) -> bool {
        self.class == class || self.class.ancestors().contains(&class)
    }
}

/// An item. Indexed by item id in [`ITEMS`].
#[derive(Debug)]
pub struct Item {
    pub name: &'static str,
    /// The default state of the block with the same name, if there is one.
    ///
    /// Items that place a differently named block, such as seeds, have `None` here.
    pub block: Option<BlockState>,
    /// The item's class in the game, which says how it places a block: as it is, as
    /// two blocks one above the other, or on a floor or a wall.
    pub class: ItemClass,
    /// The id of the block the item places, if it places one; for an item that places
    /// a standing or a wall block, the standing one.
    pub places: Option<u16>,
    /// For an item that places a standing or a wall block (torches, signs, banners,
    /// heads), the id of the wall block and the direction the item attaches in: down
    /// for a block that stands on the floor, up for a sign that hangs.
    pub wall: Option<(u16, Face)>,
}

/// A registry the server sends to the client during configuration.
///
/// The position of an entry is its numeric id. Clustine lists entries in sorted order,
/// so these ids differ from the ones a vanilla server assigns.
#[derive(Debug)]
pub struct Registry {
    pub name: &'static str,
    pub entries: &'static [&'static str],
}

impl Registry {
    /// The numeric id of `entry`, for example of `minecraft:plains`.
    pub fn id_of(&self, entry: &str) -> Option<i32> {
        let index = self.entries.binary_search(&entry).ok()?;
        i32::try_from(index).ok()
    }
}

/// The synchronised registry called `name`, for example `minecraft:dimension_type`.
pub fn synced_registry(name: &str) -> Option<&'static Registry> {
    SYNCED_REGISTRIES
        .iter()
        .find(|registry| registry.name == name)
}

/// A dimension type. Indexed by its id in the `minecraft:dimension_type` registry.
#[derive(Debug)]
pub struct DimensionType {
    pub name: &'static str,
    pub min_y: i32,
    pub height: u32,
}

/// The tags of one registry, with members given as numeric ids in that registry.
#[derive(Debug)]
pub struct RegistryTags {
    pub registry: &'static str,
    pub tags: &'static [Tag],
}

#[derive(Debug)]
pub struct Tag {
    pub name: &'static str,
    pub entries: &'static [i32],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_26_3() {
        assert_eq!(GAME_VERSION, "26.3");
        assert_eq!(PROTOCOL_VERSION, 777);
    }

    #[test]
    fn well_known_ids() {
        assert_eq!(blocks::AIR, BlockState(0));
        assert_eq!(blocks::STONE, BlockState(1));
        assert_eq!(items::AIR, 0);
        assert_eq!(items::STONE, 1);
        assert_eq!(
            ENTITY_TYPES[entity_types::PLAYER as usize],
            "minecraft:player"
        );
    }

    #[test]
    fn block_states_form_one_contiguous_range() {
        let mut next = 0;
        for block in &BLOCKS {
            assert_eq!(u32::from(block.first_state.0), next, "{}", block.name);
            assert!(block.first_state <= block.default_state);
            assert!(block.default_state <= block.last_state);
            next = u32::from(block.last_state.0) + 1;
        }
        assert_eq!(next, BLOCK_STATE_COUNT);
    }

    #[test]
    fn state_maps_back_to_its_block() {
        // `snowy=false` is the default of grass blocks and the second of its two states.
        let grass = blocks::GRASS_BLOCK.block().unwrap();
        assert_eq!(grass.name, "minecraft:grass_block");
        assert_eq!(grass.last_state, blocks::GRASS_BLOCK);
        assert_eq!(
            BlockState(grass.first_state.0).block().unwrap().name,
            grass.name
        );

        let last = BlockState((BLOCK_STATE_COUNT - 1) as u16);
        assert!(last.block().is_some());
        assert!(BlockState(last.0 + 1).block().is_none());
    }

    #[test]
    fn items_know_their_block() {
        assert_eq!(ITEMS[items::STONE as usize].block, Some(blocks::STONE));
        assert_eq!(ITEMS[items::STICK as usize].block, None);
    }

    /// Which block an item places is the game's, and is not always the block of the
    /// item's name: seeds and redstone place others, a torch or a sign one of two.
    #[test]
    fn items_know_what_they_place_and_how() {
        let name = |id: Option<u16>| id.map(|id| BLOCKS[usize::from(id)].name);
        let item = |id: i32| &ITEMS[id as usize];
        let placing = |id: i32| (item(id).class, name(item(id).places));
        assert_eq!(
            placing(items::OAK_LOG),
            (ItemClass::BlockItem, Some("minecraft:oak_log"))
        );
        assert_eq!(
            placing(items::WHEAT_SEEDS),
            (ItemClass::BlockItem, Some("minecraft:wheat"))
        );
        assert_eq!(
            placing(items::REDSTONE),
            (ItemClass::BlockItem, Some("minecraft:redstone_wire"))
        );
        assert_eq!(
            placing(items::OAK_DOOR),
            (ItemClass::DoubleHighBlockItem, Some("minecraft:oak_door"))
        );
        assert_eq!(
            placing(items::RED_BED),
            (ItemClass::BlockItem, Some("minecraft:red_bed"))
        );
        assert_eq!(placing(items::WATER_BUCKET), (ItemClass::BucketItem, None));
        assert_eq!(placing(items::STICK), (ItemClass::Item, None));
        for (id, class, wall, direction) in [
            (
                items::TORCH,
                ItemClass::StandingAndWallBlockItem,
                "minecraft:wall_torch",
                Face::Down,
            ),
            (
                items::OAK_SIGN,
                ItemClass::StandingAndWallBlockItem,
                "minecraft:oak_wall_sign",
                Face::Down,
            ),
            (
                items::OAK_HANGING_SIGN,
                ItemClass::HangingSignItem,
                "minecraft:oak_wall_hanging_sign",
                Face::Up,
            ),
            (
                items::WHITE_BANNER,
                ItemClass::BannerItem,
                "minecraft:white_wall_banner",
                Face::Down,
            ),
        ] {
            assert_eq!(item(id).class, class, "{}", item(id).name);
            let (block, attached) = item(id).wall.unwrap();
            assert_eq!((name(Some(block)), attached), (Some(wall), direction));
        }
        assert_eq!(item(items::OAK_LOG).wall, None);
    }

    /// A rule written for a class holds for the classes made from it; a pot knows
    /// what it holds; a torch is clicked by a shape nothing collides with.
    #[test]
    fn blocks_know_their_classes_their_pots_and_their_outlines() {
        let info = |name: &str| BlockState::parse(name).unwrap().block_info().unwrap();
        assert!(info("minecraft:oak_door").is(BlockClass::DoorBlock));
        assert!(info("minecraft:copper_door").is(BlockClass::DoorBlock));
        assert_ne!(info("minecraft:copper_door").class, BlockClass::DoorBlock);
        assert!(info("minecraft:repeater").is(BlockClass::DiodeBlock));
        assert!(info("minecraft:comparator").is(BlockClass::DiodeBlock));
        assert!(!info("minecraft:stone").is(BlockClass::DoorBlock));
        assert!(info("minecraft:torch").survival_rule);
        assert!(!info("minecraft:stone").survival_rule);

        let fern = BlockState::parse("minecraft:fern")
            .unwrap()
            .block_id()
            .unwrap();
        assert_eq!(info("minecraft:potted_fern").potted, Some(fern as u16));
        assert_eq!(info("minecraft:flower_pot").potted, Some(0));
        assert_eq!(info("minecraft:fern").potted, None);

        let torch = blocks::TORCH.props();
        assert!(torch.collision_is_empty());
        let outline: Vec<[f64; 6]> = torch.outline_shape().boxes().collect();
        assert_eq!(outline.len(), 1);
        assert!(outline[0][4] > 0.5 && outline[0][0] > 0.0, "{outline:?}");
        assert_eq!(blocks::STONE.props().outline_shape(), Shape::FULL_BLOCK);
        assert_eq!(blocks::AIR.props().outline_shape(), Shape::EMPTY);
    }

    #[test]
    fn synced_registries_are_sorted_for_lookup() {
        assert_eq!(SYNCED_REGISTRIES.len(), 32);
        for registry in &SYNCED_REGISTRIES {
            assert!(!registry.entries.is_empty(), "{}", registry.name);
            assert!(registry.entries.is_sorted(), "{}", registry.name);
        }
        let biomes = synced_registry("minecraft:worldgen/biome").unwrap();
        let plains = biomes.id_of("minecraft:plains").unwrap();
        assert_eq!(biomes.entries[plains as usize], "minecraft:plains");
        assert_eq!(biomes.id_of("minecraft:no_such_biome"), None);
    }

    #[test]
    fn overworld_dimensions() {
        let registry = synced_registry("minecraft:dimension_type").unwrap();
        let id = registry.id_of("minecraft:overworld").unwrap();
        let overworld = &DIMENSION_TYPES[id as usize];
        assert_eq!(overworld.name, "minecraft:overworld");
        assert_eq!((overworld.min_y, overworld.height), (-64, 384));
    }

    #[test]
    fn tags_reference_existing_entries() {
        let block_tags = TAGS
            .iter()
            .find(|tags| tags.registry == "minecraft:block")
            .unwrap();
        let infiniburn = block_tags
            .tags
            .iter()
            .find(|tag| tag.name == "minecraft:infiniburn_overworld")
            .unwrap();
        let names: Vec<_> = infiniburn
            .entries
            .iter()
            .map(|&id| BLOCKS[id as usize].name)
            .collect();
        assert_eq!(names, ["minecraft:netherrack", "minecraft:magma_block"]);

        for tags in &TAGS {
            let size = match tags.registry {
                "minecraft:block" => BLOCKS.len(),
                "minecraft:item" => ITEMS.len(),
                "minecraft:entity_type" => ENTITY_TYPES.len(),
                name => match synced_registry(name) {
                    Some(registry) => registry.entries.len(),
                    None => continue,
                },
            };
            for tag in tags.tags {
                assert!(
                    tag.entries.iter().all(|&id| (id as usize) < size),
                    "{} in {}",
                    tag.name,
                    tags.registry
                );
            }
        }
    }
}
