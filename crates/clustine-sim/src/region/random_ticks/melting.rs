//! The random ticks of what melts by the blocks' light (`docs/groundwork/n8-plan.md`,
//! section 6.2): snow layers and ice. Part D2 of N8 fills it.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use crate::api::TickOutput;
use crate::region::{Random, Region};

impl Region {
    /// The random tick of `state` at `pos` if it is one of these rules', and whether it
    /// was.
    pub(super) fn melting_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        let _ = (pos, state, random, output);
        false
    }
}
