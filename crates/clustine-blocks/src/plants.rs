//! What a plant does as its neighbours change that comes with light in the simulation
//! (`docs/groundwork/n8-plan.md`, section 6.3): an attached stem whose fruit went turns
//! back, and a bamboo shoot under bamboo becomes bamboo. Part D2 of N8 fills it.

use clustine_data::{BlockState, Face};
use clustine_world::BlockPos;

use crate::BlockView;

/// `AttachedStemBlock.updateShape`: the stem as it is after its neighbour on `direction`
/// became `neighbour`, or `None` where the rule is its parent's (`VegetationBlock`'s
/// survival, which `shape.rs` applies).
pub fn attached_stem<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> Option<BlockState> {
    let _ = (view, state, pos, direction, neighbour);
    None
}

/// `BambooSaplingBlock.updateShape`: the shoot as it is after its neighbour on
/// `direction` became `neighbour`, or `None` where the rule is what `shape.rs` does of
/// every other block.
pub fn bamboo_shoot<V: BlockView + ?Sized>(
    view: &mut V,
    state: BlockState,
    pos: BlockPos,
    direction: Face,
    neighbour: BlockState,
) -> Option<BlockState> {
    let _ = (view, state, pos, direction, neighbour);
    None
}
