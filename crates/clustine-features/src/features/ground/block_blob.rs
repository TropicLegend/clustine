// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/block_blob.rs`.

//! `block_blob`: three small balls of one block, each a step down and aside from the
//! last, on ground the predicate allows (`BlockBlobFeature`): the forests' mossy rocks.

use clustine_data::BlockState;
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::read_state;
use crate::context::Context;
use crate::feature::Feature;
use crate::features::ground::shared::between_closed;
use crate::predicates::BlockPredicate;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct BlockBlob {
    state: BlockState,
    can_place_on: BlockPredicate,
}

impl BlockBlob {
    pub fn read(reader: &Reader) -> Read<Self> {
        let blob = Self {
            state: read_state(&reader.field("state")?)?,
            can_place_on: BlockPredicate::read(&reader.field("can_place_on")?)?,
        };
        reader.finish()?;
        Ok(blob)
    }
}

impl Feature for BlockBlob {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let floor = context.level.min_y() + 3;
        let mut origin = origin;
        // Down to ground it may stand on.
        while origin.y > floor && !self.can_place_on.test(context, origin.offset(0, -1, 0)) {
            origin = origin.offset(0, -1, 0);
        }
        if origin.y <= floor {
            return false;
        }
        for _ in 0..3 {
            let xr = context.random.next_i32_bounded(2);
            let yr = context.random.next_i32_bounded(2);
            let zr = context.random.next_i32_bounded(2);
            let reach = (xr + yr + zr) as f32 * 0.333 + 0.5;
            // `distSqr` is a double; the float's square widens to one.
            let reach_squared = f64::from(reach * reach);
            for pos in between_closed(origin.offset(-xr, -yr, -zr), origin.offset(xr, yr, zr)) {
                let (dx, dy, dz) = (pos.x - origin.x, pos.y - origin.y, pos.z - origin.z);
                if f64::from(dx * dx + dy * dy + dz * dz) <= reach_squared {
                    context.level.set_block(pos, self.state);
                }
            }
            let dx = -1 + context.random.next_i32_bounded(2);
            let dy = -context.random.next_i32_bounded(2);
            let dz = -1 + context.random.next_i32_bounded(2);
            origin = origin.offset(dx, dy, dz);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn rock() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:forest_rock").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn a_rock_falls_to_the_ground_and_is_made_of_mossy_cobblestone_around_it() {
        let rock = rock();
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        assert!(world.with_context(5, |context| rock.place(context, BlockPos::new(8, 80, 8))));
        assert!(!world.writes.is_empty());
        for (pos, state) in &world.writes {
            assert_eq!(*state, blocks::MOSSY_COBBLESTONE);
            // The first ball sits at 65, and each next one is at most one lower.
            assert!((61..=66).contains(&pos.y), "{pos:?}");
            assert!((pos.x - 8).abs() <= 4 && (pos.z - 8).abs() <= 4, "{pos:?}");
        }
        assert!(
            world
                .writes
                .iter()
                .any(|(pos, _)| *pos == BlockPos::new(8, 65, 8))
        );
    }

    #[test]
    fn a_rock_over_no_ground_it_may_stand_on_is_not_placed() {
        let rock = rock();
        let mut world = World::flat(blocks::WATER, 64);
        assert!(!world.with_context(5, |context| rock.place(context, BlockPos::new(8, 80, 8))));
        assert!(world.writes.is_empty());
    }
}
