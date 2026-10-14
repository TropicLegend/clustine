//! What placing asks of the game's data: classes, tags, items and properties.

use clustine_data::{
    BLOCK_INFO, BLOCKS, BlockClass, BlockInfo, BlockState, FLUIDS, ITEMS, Item, TAGS, blocks,
};

/// What the game says of the block of `state`. Every state a rule reads came from a
/// chunk or from this crate's tables, which hold only states of blocks.
pub(crate) fn info(state: BlockState) -> &'static BlockInfo {
    state
        .block_info()
        .expect("every state placing reads is a block's state")
}

/// The id of the block of `state`.
pub(crate) fn block_id(state: BlockState) -> usize {
    state
        .block_id()
        .expect("every state placing reads is a block's state")
}

/// Whether the block of `state` is of `class` or of a class made from it.
pub(crate) fn is(state: BlockState, class: BlockClass) -> bool {
    info(state).is(class)
}

/// The first of `rule`'s answers for the block's own class and then the classes it is
/// made from, nearest first: where rules for two classes could match a block, the one
/// for the nearer class wins (section 3.3).
pub(crate) fn nearest<T>(info: &BlockInfo, rule: impl Fn(BlockClass) -> Option<T>) -> Option<T> {
    std::iter::once(info.class)
        .chain(info.class.ancestors().iter().copied())
        .find_map(rule)
}

/// Whether two states are of the same block.
pub(crate) fn same_block(a: BlockState, b: BlockState) -> bool {
    a.block_id() == b.block_id()
}

/// The default state of the block with id `block`.
pub(crate) fn default_state(block: u16) -> BlockState {
    BLOCKS[usize::from(block)].default_state
}

/// The members of the tag `name` of `registry`, as ids in that registry.
fn tag(registry: &str, name: &str) -> &'static [i32] {
    TAGS.iter()
        .find(|tags| tags.registry == registry)
        .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
        .map(|tag| tag.entries)
        .unwrap_or_else(|| panic!("the game's data has the tag {name} of {registry}"))
}

/// Whether the block of `state` is in the block tag `name`, such as `minecraft:walls`.
pub(crate) fn in_tag(state: BlockState, name: &str) -> bool {
    let id = block_id(state) as i32;
    tag("minecraft:block", name).contains(&id)
}

/// Whether the fluid in `state` is in the fluid tag `name`, such as `minecraft:water`.
pub(crate) fn fluid_in_tag(state: BlockState, name: &str) -> bool {
    state
        .props()
        .fluid()
        .is_some_and(|fluid| tag("minecraft:fluid", name).contains(&i32::from(fluid.fluid)))
}

/// The name of the fluid in `state`, if it holds one.
fn fluid_name(state: BlockState) -> Option<&'static str> {
    let fluid = state.props().fluid()?;
    FLUIDS.get(usize::from(fluid.fluid)).copied()
}

/// Whether `state` holds a source of water: water itself or a waterlogged block. This
/// is vanilla's `getFluidState().getType() == Fluids.WATER`, which flowing water is not.
pub(crate) fn water_source(state: BlockState) -> bool {
    fluid_name(state) == Some("minecraft:water")
        && state.props().fluid().is_some_and(|fluid| fluid.source)
}

/// Whether `state` holds water of any kind, vanilla's `is(FluidTags.WATER)`.
pub(crate) fn has_water(state: BlockState) -> bool {
    fluid_in_tag(state, "minecraft:water")
}

/// Whether `state` holds lava of any kind.
pub(crate) fn has_lava(state: BlockState) -> bool {
    fluid_in_tag(state, "minecraft:lava")
}

/// The item with id `id`.
pub(crate) fn item(id: i32) -> Option<&'static Item> {
    usize::try_from(id).ok().and_then(|id| ITEMS.get(id))
}

/// Whether `held` is the item of the block of `state`, vanilla's
/// `stack.is(block.asItem())`. An item that places a standing or a wall block is the
/// item of both.
pub(crate) fn is_item_of(held: Option<i32>, state: BlockState) -> bool {
    let Some(item) = held.and_then(item) else {
        return false;
    };
    let block = block_id(state) as u16;
    item.places == Some(block) || item.wall.is_some_and(|(wall, _)| wall == block)
}

/// The value of the property `name` of `state` as a number.
pub(crate) fn number(state: BlockState, name: &str) -> Option<u8> {
    state.property(name)?.parse().ok()
}

/// Whether the boolean property `name` of `state` is true.
pub(crate) fn flag(state: BlockState, name: &str) -> bool {
    state.property(name) == Some("true")
}

/// `state` with the property `name` set to `value`, or `state` itself if its block has
/// no such property: rules set what a class has, and some blocks of a class lack one.
pub(crate) fn set(state: BlockState, name: &str, value: &str) -> BlockState {
    state.with_property(name, value).unwrap_or(state)
}

/// `state` with the boolean property `name` set.
pub(crate) fn set_flag(state: BlockState, name: &str, value: bool) -> BlockState {
    set(state, name, if value { "true" } else { "false" })
}

/// `state` with the number property `name` set.
pub(crate) fn set_number(state: BlockState, name: &str, value: u8) -> BlockState {
    set(state, name, &value.to_string())
}

/// The block of a flower pot that holds the block `plant`, if a pot holds it.
pub(crate) fn pot_holding(plant: u16) -> Option<BlockState> {
    BLOCK_INFO
        .iter()
        .zip(BLOCKS.iter())
        .find(|(info, _)| plant != 0 && info.potted == Some(plant))
        .map(|(_, block)| block.default_state)
}

/// Air, which a rule reads for a block it only looks at in a chunk it cannot see.
pub(crate) const AIR: BlockState = blocks::AIR;

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_data::items;

    #[test]
    fn a_torch_is_the_item_of_the_wall_torch_too() {
        assert!(is_item_of(Some(items::TORCH), blocks::TORCH));
        assert!(is_item_of(Some(items::TORCH), blocks::WALL_TORCH));
        assert!(!is_item_of(Some(items::STONE), blocks::TORCH));
        assert!(!is_item_of(None, blocks::AIR));
    }

    #[test]
    fn water_is_a_source_and_flowing_water_is_not() {
        assert!(water_source(blocks::WATER));
        let flowing = blocks::WATER.with_property("level", "3").unwrap();
        assert!(!water_source(flowing));
        assert!(has_water(flowing));
        assert!(!water_source(blocks::LAVA));
        assert!(has_lava(blocks::LAVA));
        let logged = set_flag(blocks::OAK_STAIRS, "waterlogged", true);
        assert!(water_source(logged));
    }

    #[test]
    fn a_pot_is_found_for_what_it_holds() {
        let fern = block_id(blocks::FERN) as u16;
        assert_eq!(pot_holding(fern), Some(blocks::POTTED_FERN));
        assert_eq!(pot_holding(block_id(blocks::STONE) as u16), None);
    }
}
