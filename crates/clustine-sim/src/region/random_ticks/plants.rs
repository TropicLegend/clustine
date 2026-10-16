//! The random ticks of plants that need no light (step B7): leaves that decay, sugar
//! cane, cactus and its flower, kelp, cave vines, weeping and twisting vines, vines,
//! cocoa, nether wart, the chorus flower and a hanging mangrove propagule's age.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use super::Random;
use crate::api::TickOutput;
use crate::region::Region;

impl Region {
    /// The random tick of `state` at `pos` if it is of this file's blocks; whether it
    /// was.
    pub(super) fn plant_random_tick(
        &mut self,
        _pos: BlockPos,
        _state: BlockState,
        _random: &mut Random,
        _output: &mut TickOutput,
    ) -> bool {
        false
    }
}
