//! The daylight detector (`DaylightDetectorBlock`, its block entity's ticker;
//! `docs/groundwork/n8-plan.md`, section 6.4): every 20 ticks its power from the sky's
//! light and the sun's angle, inverted or not, and a click turning it. Part D3 of N8
//! fills it.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// The ticker of the detector at `pos`, whose block is `state`.
    pub(crate) fn daylight_detector_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let _ = (pos, state, output);
    }
}
