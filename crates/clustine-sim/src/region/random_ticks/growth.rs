//! The random ticks of what grows by light (`docs/groundwork/n8-plan.md`, section 6.2):
//! crops and stems, sweet berries, beetroot and the torchflower, grass and mycelium,
//! bamboo, mushrooms, saplings and the propagule on the ground. Part D2 of N8 fills it;
//! saplings are N8f's.

use clustine_blocks::data::{flag, is_class};
use clustine_data::{BlockClass, BlockState};
use clustine_world::BlockPos;

use crate::api::TickOutput;
use crate::region::{Random, Region};

impl Region {
    /// The random tick of `state` at `pos` if it is one of these rules', and whether it
    /// was.
    pub(super) fn growth_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        let _ = (pos, &output);
        if is_class(state, BlockClass::MangrovePropaguleBlock) && !flag(state, "hanging") {
            // `advanceTree` comes with N8f; the chance is drawn all the same, so that what
            // follows in the stream draws as the game's would.
            let _ = random.next_int(7);
            return true;
        }
        false
    }
}
