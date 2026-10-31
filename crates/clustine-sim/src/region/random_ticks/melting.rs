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
    /// (the Nether) the ice only goes: `removeBlock(pos, false)`, which leaves air with
    /// every update, and nothing is told or posted besides (n8 IceBlock 91-110, `melt`'s
    /// first branch).
    fn ice_random_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let dampening = i32::from(state.props().light_dampening());
        if self.block_light(pos) <= 11 - dampening {
            return;
        }
        if self.water_evaporates() {
            // Ice holds no fluid, so what `removeBlock` leaves is air.
            self.set_block(pos, blocks::AIR, output);
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

#[cfg(test)]
mod tests {
    use clustine_data::DIMENSION_TYPES;
    use clustine_world::{Biome, Chunk, ChunkPos, Dimension, EntityIds, Vec3};

    use super::*;
    use crate::api::{Inventory, RegionEvent};
    use crate::region::{Holdings, RegionConfig};

    /// Where the ice stands, on a stone floor, within every dimension's height.
    const ICE: BlockPos = BlockPos::new(8, 65, 8);

    /// A region of `dimension` with the 3×3 chunks around the origin loaded, a stone
    /// floor under `ICE`, ice there and a torch `away` blocks off along x, lit.
    fn lit_ice(dimension: Dimension, away: usize) -> Region {
        let config = RegionConfig {
            world_spawn: clustine_world::BlockPos::new(0, -60, 0),
            strongholds: Vec::new(),
            homes: Vec::new(),
            dimension,
            spawn: Some(Vec3::new(0.5, 66.0, 0.5)),
            starting_inventory: Inventory::default(),
            return_after: 0,
            lowest_y: dimension.rules().min_y,
            seed: 0,
            game_seed: 0,
            sea_level: 63,
            spawn_mobs: false,
            light: Default::default(),
            features: Default::default(),
            random_tick_speed: crate::region::RANDOM_TICK_SPEED,
        };
        let ids = EntityIds::block(0).expect("the first block of ids");
        let mut region = Region::new(config, ids, Holdings::default());
        let kind = DIMENSION_TYPES
            .iter()
            .find(|kind| kind.name == dimension.type_name())
            .expect("the dimension's type");
        for x in -1..=1 {
            for z in -1..=1 {
                let mut chunk = Chunk::empty(kind, Biome(0));
                for bx in 0..16 {
                    for bz in 0..16 {
                        chunk.set(bx, ICE.y - 1, bz, blocks::STONE);
                    }
                }
                if (x, z) == (0, 0) {
                    chunk.set(8, ICE.y, 8, blocks::ICE);
                    chunk.set(8 + away, ICE.y, 8, blocks::TORCH);
                }
                region.chunks.insert(ChunkPos::new(x, z), chunk);
            }
        }
        region.update_ticking();
        region.light_step();
        region
    }

    /// The random tick of the ice; what it sent.
    fn melt(region: &mut Region) -> TickOutput {
        let mut random = Random::of_section(1, ChunkPos::new(0, 0), 0, 1);
        let mut output = TickOutput::default();
        assert!(region.melting_random_tick(ICE, blocks::ICE, &mut random, &mut output));
        output
    }

    #[test]
    fn ice_beside_a_torch_melts_to_water_where_water_does_not_evaporate() {
        for dimension in [Dimension::Overworld, Dimension::End] {
            let mut region = lit_ice(dimension, 1);
            melt(&mut region);
            assert_eq!(region.block(ICE), Some(blocks::WATER), "{dimension:?}");
        }
    }

    #[test]
    fn ice_beside_a_torch_in_the_nether_melts_to_air_and_leaves_no_water() {
        let mut region = lit_ice(Dimension::Nether, 1);
        let output = melt(&mut region);
        assert_eq!(region.block(ICE), Some(blocks::AIR));
        assert!(output.events.iter().any(|event| matches!(
            event,
            RegionEvent::BlockChanged { position, .. } if *position == ICE
        )));
    }

    #[test]
    fn ice_far_from_a_torch_stays_in_every_dimension() {
        for dimension in Dimension::ALL {
            let mut region = lit_ice(dimension, 6);
            melt(&mut region);
            assert_eq!(region.block(ICE), Some(blocks::ICE), "{dimension:?}");
        }
    }
}
