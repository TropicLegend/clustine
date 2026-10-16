//! Vanilla's `canSurvive`: whether a block can stay where it is placed, by its class
//! (section 4.3), from `clustine-blocks`, which holds the one set of these rules world
//! generation shares. A block whose class has a `canSurvive` that is not ported for
//! placing places nothing.

use clustine_data::BlockState;
use clustine_world::BlockPos;

#[cfg(test)]
pub(crate) use clustine_blocks::Survival;
pub(crate) use clustine_blocks::data::facing;
pub(crate) use clustine_blocks::{multiface_can_attach, sturdy};

use super::context::{View, World};

/// The rule of the nearest class that has one, for a block whose class says where it
/// can stay; `None` for a block that can stay anywhere. Only the tests ask it apart
/// from [`can_survive`].
#[cfg(test)]
pub(crate) fn survival_rule(state: BlockState) -> Option<Survival> {
    clustine_blocks::survival_rule(state, clustine_blocks::Rules::Placing)
}

/// Whether `state` can stay at `pos`.
pub(crate) fn can_survive(state: BlockState, pos: BlockPos, world: &World) -> bool {
    clustine_blocks::can_survive(state, pos, &mut View(world))
}

/// Vanilla's `ScaffoldingBlock.getDistance`: how many blocks of scaffolding lie between
/// `pos` and a block that holds it up.
pub(crate) fn scaffolding_distance(pos: BlockPos, world: &World) -> u8 {
    clustine_blocks::survive::scaffolding_distance(pos, &mut View(world))
}
