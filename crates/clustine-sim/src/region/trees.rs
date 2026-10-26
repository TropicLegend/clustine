//! Saplings and the trees they grow (`docs/groundwork/n8-plan.md`, section 7.1):
//! `SaplingBlock.advanceTree` and the game's `TreeGrower`, which places its tree's
//! feature through the region's view (`feature_view.rs`). Part D4a of N8 fills it; bone
//! meal (`bone_meal.rs`, part D4b) calls [`Region::advance_tree`] for a sapling's and a
//! propagule's success.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use super::{Random, Region};
use crate::api::TickOutput;

impl Region {
    /// `SaplingBlock.advanceTree` of the sapling or propagule `state` at `pos`: a sapling
    /// of stage 0 takes stage 1 (flags 260); one of stage 1 grows its grower's tree,
    /// drawing the sapling's own chances from `random` and the tree's from a random seeded
    /// with `random`'s next long (the plan's 7.3).
    #[expect(dead_code, reason = "saplings and bone meal call it (N8 D4a, D4b)")]
    pub(crate) fn advance_tree(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let _ = (pos, state, random, output);
    }
}
