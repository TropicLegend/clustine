//! `random_neighbor_spread`: a block at the origin, then the same block at random
//! places around it that touch exactly one block of the accepted ones
//! (`RandomNeighborSpreadFeature`, 26.x's glowstone feature made general, read from
//! `d-javap/net.minecraft.world.level.levelgen.feature.RandomNeighborSpreadFeature.txt`;
//! the offsets below are its bytecode's): the Nether's glowstone under its ceilings
//! and the crimson forest's nether wart blocks that its weeping vines hang from.

use clustine_data::Face;
use clustine_world::BlockPos;

use crate::blocks::{BlockSet, relative};
use crate::context::Context;
use crate::feature::Feature;
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

#[derive(Debug)]
pub struct RandomNeighborSpread {
    block: Provider,
    accepted_neighbors: BlockSet,
    can_replace: BlockPredicate,
    attempts: IntProvider,
    xz_offset: IntProvider,
    y_offset: IntProvider,
}

impl RandomNeighborSpread {
    pub fn read(reader: &Reader) -> Read<Self> {
        let spread = Self {
            block: Provider::read(&reader.field("block")?)?,
            accepted_neighbors: BlockSet::read(&reader.field("accepted_neighbors")?)?,
            can_replace: BlockPredicate::read(&reader.field("can_replace")?)?,
            attempts: IntProvider::read(&reader.field("attempts")?)?,
            xz_offset: IntProvider::read(&reader.field("xz_offset")?)?,
            y_offset: IntProvider::read(&reader.field("y_offset")?)?,
        };
        reader.finish()?;
        Ok(spread)
    }
}

impl Feature for RandomNeighborSpread {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        // The origin takes the block whatever is there, unasked (0-30): the placement
        // has already asked for air under the ceiling. The provider's `getState`, not
        // `getOptionalState`, and drawn before the attempts are (49-59).
        let state = self.block.state(context, origin);
        context.level.set_block(origin, state);
        let attempts = self.attempts.sample(context.random);
        for _ in 0..attempts {
            // x, y and z drawn in that order (71-105), then the predicate at the place
            // (109-124).
            let x = self.xz_offset.sample(context.random);
            let y = self.y_offset.sample(context.random);
            let z = self.xz_offset.sample(context.random);
            let pos = origin.offset(x, y, z);
            if !self.can_replace.test(context, pos) {
                continue;
            }
            // Exactly one side touching an accepted block, in the order of
            // `Direction.values()` (127-206); the game stops counting at two, which
            // changes nothing of the answer. The block is drawn again for the place
            // (209-239), and the feature always says it was placed (246).
            let mut touching = 0;
            for face in Face::ALL {
                if self
                    .accepted_neighbors
                    .contains(context.level.block(relative(pos, face)))
                {
                    touching += 1;
                }
                if touching > 1 {
                    break;
                }
            }
            if touching == 1 {
                let state = self.block.state(context, pos);
                context.level.set_block(pos, state);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_noise::Random;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn glowstone() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:glowstone_extra").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    /// A ceiling of netherrack from `y` up, with air below it down to the ground at 0.
    fn ceiling(y: i32) -> World {
        let mut world = World::flat(blocks::NETHERRACK, 200);
        for x in -16..32 {
            for z in -16..32 {
                for under in 1..y {
                    world.set(BlockPos::new(x, under, z), blocks::AIR);
                }
            }
        }
        world.writes.clear();
        world
    }

    #[test]
    fn the_origin_takes_the_block_unasked_and_the_feature_says_it_was_placed() {
        let glowstone = glowstone();
        // Solid netherrack: the origin is not air and no attempt finds air, yet the
        // origin is glowstone and the feature placed.
        let mut world = World::flat(blocks::NETHERRACK, 200);
        let origin = BlockPos::new(8, 99, 8);
        assert!(world.with_context(5, |context| glowstone.place(context, origin)));
        assert_eq!(world.writes, vec![(origin, blocks::GLOWSTONE)]);
    }

    #[test]
    fn glowstone_hangs_from_its_origin_each_block_touching_exactly_one_before() {
        let glowstone = glowstone();
        let mut world = ceiling(100);
        let origin = BlockPos::new(8, 99, 8);
        assert!(world.with_context(5, |context| glowstone.place(context, origin)));
        assert_eq!(world.writes[0], (origin, blocks::GLOWSTONE));
        assert!(world.writes.len() > 1);
        let mut placed = vec![origin];
        for (pos, state) in &world.writes[1..] {
            assert_eq!(*state, blocks::GLOWSTONE);
            // Within the offsets: x and z from -7 to 7, y from -11 to 0.
            assert!((pos.x - 8).abs() <= 7 && (pos.z - 8).abs() <= 7, "{pos:?}");
            assert!((88..=99).contains(&pos.y), "{pos:?}");
            let touching = Face::ALL
                .iter()
                .filter(|face| placed.contains(&relative(*pos, **face)))
                .count();
            assert_eq!(touching, 1, "{pos:?}");
            placed.push(*pos);
        }
    }

    #[test]
    fn glowstone_draws_its_offsets_x_then_y_then_z_for_each_attempt() {
        let glowstone = glowstone();
        let mut world = ceiling(100);
        let origin = BlockPos::new(8, 99, 8);
        assert!(world.with_context(9, |context| glowstone.place(context, origin)));
        // The same draws by hand: the trapezoid of -7 to 7 with no plateau is the old
        // `nextInt(8) - nextInt(8)`, and the y of -11 to 0 is uniform.
        let mut random = clustine_noise::WorldgenRandom::from_seed(9);
        let mut glow = vec![origin];
        for _ in 0..1500 {
            let x = random.next_i32_bounded(8) - random.next_i32_bounded(8);
            let y = random.next_i32_bounded(12) - 11;
            let z = random.next_i32_bounded(8) - random.next_i32_bounded(8);
            let pos = origin.offset(x, y, z);
            if glow.contains(&pos) {
                continue;
            }
            let touching = Face::ALL
                .iter()
                .filter(|face| glow.contains(&relative(pos, **face)))
                .count();
            if touching == 1 {
                glow.push(pos);
            }
        }
        let written: Vec<BlockPos> = world.writes.iter().map(|(pos, _)| *pos).collect();
        assert_eq!(written, glow);
    }

    #[test]
    fn the_crimson_forest_s_wart_spreads_on_netherrack_and_wart_alike() {
        // The wart the weeping vines hang from is the first of `weeping_vines`'s two.
        let (_, entry) = data::configured("minecraft:weeping_vines").unwrap();
        let reader = Reader::new(&entry.data, entry.name);
        let first = &reader.field("features").unwrap().list().unwrap()[0];
        let wart = crate::placement::PlacedFeature::read(first).unwrap();
        let mut world = ceiling(100);
        let origin = BlockPos::new(8, 99, 8);
        assert!(world.with_context(2, |context| wart.place(context, origin)));
        assert_eq!(world.writes[0], (origin, blocks::NETHER_WART_BLOCK));
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| *state == blocks::NETHER_WART_BLOCK)
        );
        // Next to the ceiling, which is accepted as well, more than one is placed.
        assert!(world.writes.len() > 1);
    }
}
