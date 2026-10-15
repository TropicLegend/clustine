// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/blue_ice.rs`.

//! `blue_ice`: a cluster of blue ice grown from water beside packed ice under the sea
//! (`BlueIceFeature`): the blue ice of icebergs and frozen oceans.

use clustine_data::{Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{relative, same_block};
use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct BlueIce;

impl BlueIce {
    pub fn read(reader: &Reader) -> Read<Self> {
        reader.finish()?;
        Ok(Self)
    }
}

/// The reach sideways of a block `y` above or below the origin.
fn reach(y: i32) -> i32 {
    if y < 2 { 3 + y / 2 } else { 3 }
}

impl Feature for BlueIce {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let sea_level = context.settings.generation().sea_level;
        if origin.y > sea_level - 1 {
            return false;
        }
        let level = &mut *context.level;
        if !same_block(level.block(origin), blocks::WATER)
            && !same_block(level.block(relative(origin, Face::Down)), blocks::WATER)
        {
            return false;
        }
        let beside_packed_ice = Face::ALL
            .into_iter()
            .filter(|face| *face != Face::Down)
            .any(|face| same_block(level.block(relative(origin, face)), blocks::PACKED_ICE));
        if !beside_packed_ice {
            return false;
        }
        level.set_block(origin, blocks::BLUE_ICE);
        for _ in 0..200 {
            let random = &mut *context.random;
            let y = random.next_i32_bounded(5) - random.next_i32_bounded(6);
            let reach = reach(y);
            if reach < 1 {
                continue;
            }
            let x = random.next_i32_bounded(reach) - random.next_i32_bounded(reach);
            let z = random.next_i32_bounded(reach) - random.next_i32_bounded(reach);
            let pos = origin.offset(x, y, z);
            let level = &mut *context.level;
            let state = level.block(pos);
            if !(state.props().is_air()
                || same_block(state, blocks::WATER)
                || same_block(state, blocks::PACKED_ICE)
                || same_block(state, blocks::ICE))
            {
                continue;
            }
            let touches_blue_ice = Face::ALL
                .into_iter()
                .any(|face| same_block(level.block(relative(pos, face)), blocks::BLUE_ICE));
            if touches_blue_ice {
                level.set_block(pos, blocks::BLUE_ICE);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_noise::{Random, WorldgenRandom};

    use super::*;
    use crate::testing::World;

    #[test]
    fn the_reach_narrows_below_the_origin() {
        assert_eq!(reach(4), 3);
        assert_eq!(reach(1), 3);
        assert_eq!(reach(-1), 3);
        assert_eq!(reach(-2), 2);
        assert_eq!(reach(-5), 1);
    }

    #[test]
    fn blue_ice_grows_from_water_beside_packed_ice_and_stays_connected() {
        let mut world = World::flat(blocks::WATER, 62);
        let origin = BlockPos::new(8, 50, 8);
        world.set(origin.offset(1, 0, 0), blocks::PACKED_ICE);
        assert!(world.with_context(7, |context| BlueIce.place(context, origin)));
        assert_eq!(world.writes[0], (origin, blocks::BLUE_ICE));
        assert!(world.writes.len() > 1);
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| *state == blocks::BLUE_ICE)
        );
        for (pos, _) in &world.writes {
            assert!((pos.x - 8).abs() <= 2 && (pos.z - 8).abs() <= 2, "{pos:?}");
            assert!((45..=54).contains(&pos.y), "{pos:?}");
        }
    }

    #[test]
    fn blue_ice_above_the_sea_or_away_from_packed_ice_draws_nothing() {
        let mut world = World::flat(blocks::WATER, 62);
        let mut random = WorldgenRandom::from_seed(7);
        let mut expected = random.clone();
        assert!(!world.with_random(&mut random, None, |context| {
            BlueIce.place(context, BlockPos::new(8, 50, 8))
        }));
        assert!(!world.with_random(&mut random, None, |context| {
            BlueIce.place(context, BlockPos::new(8, 63, 8))
        }));
        assert_eq!(random.next_i64(), expected.next_i64());
        assert!(world.writes.is_empty());
    }
}
