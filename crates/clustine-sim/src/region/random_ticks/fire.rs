//! Fire and coral (step B7): fire's scheduled tick, burning out and spreading, lava's
//! random tick, which sets fire, and coral dying out of water.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use super::Random;
use crate::api::TickOutput;
use crate::region::Region;

impl Region {
    /// The random tick of `state` at `pos` if it is of this file's blocks; whether it
    /// was.
    pub(super) fn fire_random_tick(
        &mut self,
        _pos: BlockPos,
        _state: BlockState,
        _random: &mut Random,
        _output: &mut TickOutput,
    ) -> bool {
        false
    }

    /// The scheduled tick of fire or coral at `pos`, if `state` is one; whether it was.
    pub(crate) fn fire_block_tick(
        &mut self,
        _pos: BlockPos,
        _state: BlockState,
        _output: &mut TickOutput,
    ) -> bool {
        false
    }

    /// What fire and coral do as they are placed (`onPlace`): schedule their tick.
    pub(crate) fn fire_on_place(&mut self, _pos: BlockPos, _state: BlockState) {}
}
