//! The magma cube (`MagmaCube`): family E1 of P12 (`docs/groundwork/p-plan.md`, sections 18.3, 18.5 and
//! 18.6).
//!
//! Until E1 makes it whole, the type takes every default and is not built (`BUILT`),
//! so that no egg, spawner or structure makes one.

use clustine_world::BlockPos;

use super::MobType;
use crate::spawning::rules::SpawnWorld;
use clustine_world::generation::Draws;

/// Whether the type is whole, so that `built()` gives it: E1 sets it when it is.
pub(crate) const BUILT: bool = false;

/// The magma cube.
pub(crate) struct MagmaCube;

impl MobType for MagmaCube {}

/// The type's spawn rule at a natural reason (`SpawnPlacements`' predicate), which
/// `spawning::rules` asks: none until E1 builds it.
pub(crate) fn check_spawn_rules(
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    let _ = (world, pos, draws);
    false
}
