//! Zombified piglins from Nether portal blocks (`NetherPortalBlock.randomTick`; the
//! brief's section 4.2): family E1 of P12 (`docs/groundwork/p-plan.md`, sections 18.3,
//! 18.5 and 18.6) fills this. In a level whose `nether_portal_spawns_piglin` holds (the
//! overworld), a portal block's random tick may make one on the block under the portal,
//! on portal cooldown.
//!
//! Until E1 builds it, a portal's random tick does nothing.

use clustine_blocks::data::is_block;
use clustine_data::{BlockState, blocks};
use clustine_world::BlockPos;

use super::{Random, Region};
use crate::api::TickOutput;

impl Region {
    /// A random tick of `state` at `pos`, where it is a portal block: whether it was
    /// one, which ends the random tick's search for a rule.
    pub(super) fn portal_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        if !is_block(state, blocks::NETHER_PORTAL) {
            return false;
        }
        let _ = (pos, random, output);
        true
    }
}
