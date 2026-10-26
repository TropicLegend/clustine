//! The random ticks of what melts by the blocks' light (`docs/groundwork/n8-plan.md`,
//! section 6.2): snow layers, and ice with frosted ice, which takes `IceBlock`'s random
//! tick. Both read the blocks' light alone, so the sun melts nothing; neither draws.
//!
//! Frosted ice's own ageing is a scheduled tick, which only Frost Walker starts and
//! Clustine has no enchantment for: not built (the plan's 1.9).

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::BlockPos;

use crate::api::TickOutput;
use crate::region::vibrations::{Context, Vibration};
use crate::region::{Random, Region};

impl Region {
    /// The random tick of `state` at `pos` if it is one of these rules', and whether it
    /// was.
    pub(in crate::region) fn melting_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        let _ = random;
        if is_class(state, BlockClass::SnowLayerBlock) {
            self.snow_random_tick(pos, state, output);
        } else if is_class(state, BlockClass::IceBlock) {
            self.ice_random_tick(pos, state, output);
        } else {
            return false;
        }
        true
    }

    /// The blocks' light at `pos`, none where there is no answer.
    fn block_light(&self, pos: BlockPos) -> i32 {
        i32::from(self.block_brightness(pos).unwrap_or(0))
    }

    /// `SnowLayerBlock.randomTick` (n7 SnowLayerBlock 178–200): in block light above 11
    /// (two blocks from a torch, three from glowstone) the snow drops what it drops by
    /// its table (nothing without a tool), goes (`removeBlock`, every update) and posts
    /// `block_destroy`.
    fn snow_random_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        if self.block_light(pos) <= 11 {
            return;
        }
        self.drop_resources(state, pos, output);
        // A snow layer holds no fluid, so what `removeBlock` leaves is air.
        self.set_block(pos, blocks::AIR, output);
        if let Some(vibration) = Vibration::named("minecraft:block_destroy") {
            self.vibrate_at(vibration, pos, Context::of(None, Some(state)));
        }
    }

    /// `IceBlock.randomTick` (n8 IceBlock 63–115): in block light above 11 less the ice's
    /// own dampening it melts (`melt`): to water with every update, the water told that
    /// water came (so that it flows), and `block_destroy` posted. Where water evaporates
    /// the ice would only go, which is no place of Clustine's overworld.
    fn ice_random_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let dampening = i32::from(state.props().light_dampening());
        if self.block_light(pos) <= 11 - dampening {
            return;
        }
        self.set_block(pos, blocks::WATER, output);
        // `neighborChanged(pos, water, null)`: told by no side; the side only says where
        // an update that cannot be made is kept, and this one is made in the region's
        // own chunk.
        self.neighbour_changed_at(pos, Face::Down, blocks::WATER, false, output);
        if let Some(vibration) = Vibration::named("minecraft:block_destroy") {
            self.vibrate_at(vibration, pos, Context::of(None, Some(state)));
        }
    }
}
