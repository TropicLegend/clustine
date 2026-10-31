//! `end_island`: a small floating island of end stone, layers of discs narrowing
//! downwards (`EndIslandFeature`, read from `d-javap`; the offsets below are its
//! bytecode's). The small islands' biome places it, and a gateway that finds no land
//! for its exit makes one from a random of the exit's position (the plan, section 19.6),
//! for which [`island_blocks`] is the shape.

use clustine_data::blocks;
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};

/// The offsets from the origin that an island of `random`'s draws covers with end
/// stone, in the order the game sets them (`place`, offsets 0-142): a first radius of 4
/// to 6, then while it is above a half, a disc of the radius plus one at each layer
/// down, the radius shrinking by a half or one and a half each time.
pub fn island_blocks(random: &mut impl Random) -> Vec<(i32, i32, i32)> {
    island_of(|bound| random.next_i32_bounded(bound))
}

/// [`island_blocks`] of the draws `next_int` makes.
fn island_of(mut next_int: impl FnMut(i32) -> i32) -> Vec<(i32, i32, i32)> {
    let mut blocks = Vec::new();
    let mut radius = next_int(3) as f32 + 4.0;
    let mut y = 0;
    while radius > 0.5 {
        let reach = (radius + 1.0) * (radius + 1.0);
        // `Mth.floor(-radius)` to `Mth.ceil(radius)` on both axes.
        let (low, high) = ((-radius).floor() as i32, radius.ceil() as i32);
        for x in low..=high {
            for z in low..=high {
                if (x * x + z * z) as f32 <= reach {
                    blocks.push((x, y, z));
                }
            }
        }
        radius -= next_int(2) as f32 + 0.5;
        y -= 1;
    }
    blocks
}

#[derive(Debug)]
pub struct EndIsland;

impl EndIsland {
    /// The kind has no settings.
    pub fn read(reader: &Reader) -> Read<Self> {
        reader.finish()?;
        Ok(Self)
    }
}

impl Feature for EndIsland {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        for (x, y, z) in island_blocks(context.random) {
            context
                .level
                .set_block(origin.offset(x, y, z), blocks::END_STONE);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_noise::{LegacyRandom, WorldgenRandom};

    use super::*;
    use crate::testing::World;

    #[test]
    fn an_island_narrows_downwards_from_a_disc_of_four_to_six_across() {
        for seed in 0..50 {
            let blocks = island_blocks(&mut LegacyRandom::from_seed(seed));
            let top: Vec<_> = blocks.iter().filter(|(_, y, _)| *y == 0).collect();
            let widest = top.iter().map(|(x, _, _)| x.abs()).max().unwrap();
            // A first radius of 4, 5 or 6: the disc of one more is cut at the radius,
            // rounded up, on each axis.
            assert!((4..=6).contains(&widest), "seed {seed}: {widest}");
            // Each layer is no wider than the one above it.
            let lowest = blocks.iter().map(|(_, y, _)| *y).min().unwrap();
            let mut above = usize::MAX;
            for y in (lowest..=0).rev() {
                let layer = blocks.iter().filter(|(_, at, _)| *at == y).count();
                assert!(layer > 0 && layer <= above, "seed {seed}");
                above = layer;
            }
        }
    }

    #[test]
    fn an_island_of_radius_four_shrinking_by_halves_is_the_game_s_discs() {
        // Draws of nought every time: radius 4, then 3.5, 3, ... down to 1.
        let blocks = island_of(|_| 0);
        let layer = |y: i32| blocks.iter().filter(|(_, at, _)| *at == y).count();
        // Radius 4: every x, z in -4..=4 within 5 of the middle: all 81 but the four
        // corners, which are 4√2 away; (4, 3) is 5 away exactly and in.
        assert_eq!(layer(0), 81 - 4);
        // Seven layers: 4, 3.5, 3, 2.5, 2, 1.5, 1; then 0.5 ends it.
        assert_eq!(blocks.iter().map(|(_, y, _)| *y).min(), Some(-6));
        // Radius 1: within 2 of the middle among -1..=1, every one.
        assert_eq!(layer(-6), 9);
    }

    #[test]
    fn the_feature_sets_end_stone_where_its_shape_says() {
        let mut world = World::flat(blocks::AIR, -100);
        let origin = BlockPos::new(8, 60, 8);
        world.with_context(3, |context| {
            assert!(EndIsland.place(context, origin));
        });
        let expected = island_blocks(&mut WorldgenRandom::from_seed(3));
        assert_eq!(world.writes.len(), expected.len());
        for ((at, state), (x, y, z)) in world.writes.iter().zip(expected) {
            assert_eq!(*at, origin.offset(x, y, z));
            assert_eq!(*state, blocks::END_STONE);
        }
    }
}
