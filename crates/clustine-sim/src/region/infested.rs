//! Infested blocks (`InfestedBlock`; `docs/groundwork/n9-plan.md`, section 8.4): part
//! S9's.
//!
//! Until S9 fills it, an infested block broken makes no silverfish.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// `InfestedBlock.spawnAfterBreak`: what comes of the block `state` broken at `pos`
    /// (by a rule, after its drops, or by an explosion, before them): a silverfish where
    /// it is an infested block.
    #[expect(
        clippy::unused_self,
        reason = "part S9 fills it (docs/groundwork/n9-plan.md, section 8.4)"
    )]
    pub(crate) fn infested_broken(
        &mut self,
        _pos: BlockPos,
        _state: BlockState,
        _output: &mut TickOutput,
    ) {
    }
}
