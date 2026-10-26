//! What a plant does as its neighbours change that comes with light in the simulation
//! (`docs/groundwork/n8-plan.md`, section 6.3): an attached stem whose fruit went turns
//! back, and a bamboo shoot under bamboo becomes bamboo. Without the first, the first
//! fruit harvested would leave a stem attached to air for ever; without the second, a
//! shoot that grew bamboo above it would stay a shoot under a stalk.

use clustine_data::{BlockState, Face, blocks};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::data::{facing, is_block, set};
use crate::survive::can_survive;

/// `AttachedStemBlock.updateShape` (n8 AttachedStemBlock 54–100): the stem as it is after
/// its neighbour on `direction` became `neighbour`, or `None` where the rule is its
/// parent's (`VegetationBlock`'s survival, on its stem's ground, which `shape.rs`
/// applies). From the side it faces, where the neighbour is no longer its fruit, it is
/// its stem's default state at age 7 again, which fruits again.
pub fn attached_stem<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> Option<BlockState> {
    let _ = (view, pos);
    let (fruit, stem) = match state.block()?.name {
        "minecraft:attached_melon_stem" => (blocks::MELON, blocks::MELON_STEM),
        "minecraft:attached_pumpkin_stem" => (blocks::PUMPKIN, blocks::PUMPKIN_STEM),
        _ => return None,
    };
    if !is_block(neighbour, fruit) && facing(state) == Some(direction) {
        // `trySetValue(AGE, 7)` of the stem's default state.
        let stem = stem.block().map_or(stem, |block| block.default_state);
        return Some(set(stem, "age", "7"));
    }
    None
}

/// `BambooSaplingBlock.updateShape` (n8 BambooSaplingBlock 55–85): the shoot as it is
/// after its neighbour on `direction` became `neighbour`: air where it cannot stay;
/// bamboo's default state where bamboo is now above it; itself otherwise.
pub fn bamboo_shoot<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> Option<BlockState> {
    if !can_survive(state, pos, view) {
        return Some(blocks::AIR);
    }
    if direction == Face::Up && is_block(neighbour, blocks::BAMBOO) {
        let bamboo = blocks::BAMBOO;
        return Some(bamboo.block().map_or(bamboo, |block| block.default_state));
    }
    Some(state)
}
