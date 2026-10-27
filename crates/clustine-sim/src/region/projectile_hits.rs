//! What a projectile does to the block it hits (`docs/groundwork/n9-plan.md`, section
//! 4.2, step 4; part S2's): the blocks' `onProjectileHit` (a target block's signal, a
//! bell's ring, TNT lit by a burning arrow, a chorus flower broken, a pointed dripstone, a
//! decorated pot), in another region's land a `SetIf` or the block's effect as ADR-0025
//! has them; and an arrow's part in a button's `checkPressed`.
//!
//! The lead fixed the signatures in N9b: every projectile's tick calls
//! [`Region::projectile_hit`] at a block hit, before its own `onHitBlock`, and the button's
//! tick in `redstone.rs` asks [`Region::arrow_on_button`]. Until S2 fills them, a block is
//! not changed by a hit and no arrow keeps a button pressed.

use clustine_data::BlockState;
use clustine_world::{BlockPos, Entity};

use super::Region;
use crate::api::TickOutput;
use crate::physics::thrown::BlockHit;

impl Region {
    /// `state.onProjectileHit(level, state, hit, projectile)` of the block `hit.pos`, in
    /// state `state`, hit by `projectile` as it is after the step that hit (its kind,
    /// owner, fire and motion are read from it).
    #[expect(
        clippy::unused_self,
        reason = "part S2 fills it (docs/groundwork/n9-plan.md, section 4.2)"
    )]
    pub(crate) fn projectile_hit(
        &mut self,
        _projectile: &Entity,
        _state: BlockState,
        _hit: &BlockHit,
        _output: &mut TickOutput,
    ) {
    }

    /// `ButtonBlock.checkPressed`'s look for arrows: whether an arrow's box meets the
    /// button at `pos`, in state `state`, so that it stays pressed (a wooden button
    /// only).
    #[expect(
        clippy::unused_self,
        reason = "part S2 fills it (docs/groundwork/n9-plan.md, section 4.2)"
    )]
    pub(crate) fn arrow_on_button(&self, _pos: BlockPos, _state: BlockState) -> bool {
        false
    }
}
