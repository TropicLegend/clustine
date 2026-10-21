//! What a copper golem's tick reads of a chest through its view of the region
//! (`crate::mobs::types::golem_transport`): the slots and the openers' count its block
//! entity's data keeps, as the containers' rules read them (`containers.rs`), which the
//! golem's code reaches only through here.

use clustine_data::BlockState;
use clustine_world::ItemStack;

use super::containers::{container_size, read_container, read_openers};

/// The slots of the container block `state`, its block entity's data being `data`; none
/// for a block that keeps no items. A loot table not yet rolled reads as empty slots, as
/// the region reads it (N6's decision 4).
pub(crate) fn slots(state: BlockState, data: &[u8]) -> Option<Vec<Option<ItemStack>>> {
    let size = container_size(state)?;
    Some(read_container(size, data).slots)
}

/// The openers' count the block entity's `data` keeps, as its counter last counted them
/// (`getEntitiesWithContainerOpen` is not empty where it is above nothing).
pub(crate) fn openers(data: &[u8]) -> u32 {
    read_openers(data)
}

/// The block entity's data of a chest of `state` with `slots` and `openers`, as the
/// region writes it: what the golem's tests lay their chests with.
#[cfg(test)]
pub(crate) fn chest_data(state: BlockState, slots: &[Option<ItemStack>], openers: u32) -> Vec<u8> {
    let container = super::containers::Container {
        slots: slots.to_vec(),
        openers,
        loot: false,
    };
    super::containers::container_data(state, &container, &[])
}
