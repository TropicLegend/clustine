//! The random ticks of what is not a plant and needs no light (step B7): copper that
//! weathers, budding amethyst, pointed dripstone and the sulfur spike, redstone ore,
//! nylium, farmland's moisture and dried ghasts.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use super::Random;
use crate::api::TickOutput;
use crate::region::Region;

impl Region {
    /// The random tick of `state` at `pos` if it is of this file's blocks; whether it
    /// was.
    pub(super) fn mineral_random_tick(
        &mut self,
        _pos: BlockPos,
        _state: BlockState,
        _random: &mut Random,
        _output: &mut TickOutput,
    ) -> bool {
        false
    }
}
