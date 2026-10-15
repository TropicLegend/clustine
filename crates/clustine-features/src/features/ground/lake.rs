// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/lake.rs`.

//! `lake`: a pool of a fluid in a box of 16 by 8 by 16 blocks around the origin, made
//! of a few overlapping ellipsoids, with air above the fluid, a barrier around it and,
//! for water in the cold, ice on top (`LakeFeature`): the lava lakes and the sulfur
//! caves' pools.
//!
//! The air it leaves is ticked as the game ticks it, and the blocks above the air and
//! the barrier are marked to take their shape when the chunk is finished.

use clustine_data::{BlockState, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::has_water;
use crate::context::Context;
use crate::feature::Feature;
use crate::features::ground::shared::{mark_above_for_post_processing, should_freeze};
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct Lake {
    fluid: Provider,
    barrier: Provider,
    can_place_feature: BlockPredicate,
    can_replace_with_air_or_fluid: BlockPredicate,
    can_replace_with_barrier: BlockPredicate,
}

impl Lake {
    pub fn read(reader: &Reader) -> Read<Self> {
        let lake = Self {
            fluid: Provider::read(&reader.field("fluid")?)?,
            barrier: Provider::read(&reader.field("barrier")?)?,
            can_place_feature: BlockPredicate::read(&reader.field("can_place_feature")?)?,
            can_replace_with_air_or_fluid: BlockPredicate::read(
                &reader.field("can_replace_with_air_or_fluid")?,
            )?,
            can_replace_with_barrier: BlockPredicate::read(
                &reader.field("can_replace_with_barrier")?,
            )?,
        };
        reader.finish()?;
        Ok(lake)
    }
}

/// Which blocks of the box the lake fills, by `(x * 16 + z) * 8 + y`.
struct Grid([bool; 2048]);

impl Grid {
    fn index(x: i32, y: i32, z: i32) -> usize {
        usize::try_from((x * 16 + z) * 8 + y).expect("a block of the lake's box")
    }

    fn get(&self, x: i32, y: i32, z: i32) -> bool {
        self.0[Self::index(x, y, z)]
    }

    /// A block outside the lake beside one inside it, across a face.
    fn is_edge(&self, x: i32, y: i32, z: i32) -> bool {
        !self.get(x, y, z)
            && ((x < 15 && self.get(x + 1, y, z))
                || (x > 0 && self.get(x - 1, y, z))
                || (z < 15 && self.get(x, y, z + 1))
                || (z > 0 && self.get(x, y, z - 1))
                || (y < 7 && self.get(x, y + 1, z))
                || (y > 0 && self.get(x, y - 1, z)))
    }
}

/// The blocks of the box in the game's order: x, then z, then y fastest.
fn box_blocks() -> impl Iterator<Item = (i32, i32, i32)> {
    (0..16).flat_map(|x| (0..16).flat_map(move |z| (0..8).map(move |y| (x, y, z))))
}

impl Lake {
    /// Whether the lake may stand here: no fluid beside its air, nothing but solid
    /// blocks or its own fluid beside its fluid, and nothing the settings refuse.
    fn fits(
        &self,
        context: &mut Context<'_, '_>,
        grid: &Grid,
        origin: BlockPos,
        fluid: BlockState,
    ) -> bool {
        for (x, y, z) in box_blocks() {
            if !grid.is_edge(x, y, z) {
                continue;
            }
            let pos = origin.offset(x, y, z);
            let state = context.level.block(pos);
            if y >= 4 && state.props().fluid().is_some() {
                return false;
            }
            if y < 4 && !state.props().is_solid() && state != fluid {
                return false;
            }
            if !self.can_place_feature.test(context, pos) {
                return false;
            }
        }
        true
    }
}

impl Feature for Lake {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if origin.y <= context.level.min_y() + 4 {
            return false;
        }
        let origin = origin.offset(-8, -4, -8);
        let mut grid = Grid([false; 2048]);
        let spots = context.random.next_i32_bounded(4) + 4;
        for _ in 0..spots {
            let random = &mut *context.random;
            let xr = random.next_f64() * 6.0 + 3.0;
            let yr = random.next_f64() * 4.0 + 2.0;
            let zr = random.next_f64() * 6.0 + 3.0;
            let xp = random.next_f64() * (16.0 - xr - 2.0) + 1.0 + xr / 2.0;
            let yp = random.next_f64() * (8.0 - yr - 4.0) + 2.0 + yr / 2.0;
            let zp = random.next_f64() * (16.0 - zr - 2.0) + 1.0 + zr / 2.0;
            for x in 1..15 {
                for z in 1..15 {
                    for y in 1..7 {
                        let xd = (f64::from(x) - xp) / (xr / 2.0);
                        let yd = (f64::from(y) - yp) / (yr / 2.0);
                        let zd = (f64::from(z) - zp) / (zr / 2.0);
                        if xd * xd + yd * yd + zd * zd < 1.0 {
                            grid.0[Grid::index(x, y, z)] = true;
                        }
                    }
                }
            }
        }

        let fluid = self.fluid.state(context, origin);
        if !self.fits(context, &grid, origin, fluid) {
            return false;
        }

        for (x, y, z) in box_blocks() {
            if !grid.get(x, y, z) {
                continue;
            }
            let pos = origin.offset(x, y, z);
            if !self.can_replace_with_air_or_fluid.test(context, pos) {
                continue;
            }
            if y >= 4 {
                context.level.set_block(pos, blocks::CAVE_AIR);
                let air = crate::blocks::block_id(blocks::CAVE_AIR);
                context.level.schedule_block_tick(
                    pos,
                    u16::try_from(air).expect("a block's id fits a tick"),
                    0,
                );
                mark_above_for_post_processing(context.level, pos);
            } else {
                context.level.set_block(pos, fluid);
            }
        }

        let barrier = self.barrier.state(context, origin);
        if !barrier.props().is_air() {
            for (x, y, z) in box_blocks() {
                if !grid.is_edge(x, y, z) || (y >= 4 && context.random.next_i32_bounded(2) == 0) {
                    continue;
                }
                let pos = origin.offset(x, y, z);
                if context.level.block(pos).props().is_solid()
                    && self.can_replace_with_barrier.test(context, pos)
                {
                    context.level.set_block(pos, barrier);
                    mark_above_for_post_processing(context.level, pos);
                }
            }
        }

        if has_water(fluid) {
            let sea_level = context.settings.generation().sea_level;
            for x in 0..16 {
                for z in 0..16 {
                    let pos = origin.offset(x, 4, z);
                    let biome = context.level.biome(pos);
                    if should_freeze(context.level, biome, pos, sea_level, false)
                        && self.can_replace_with_air_or_fluid.test(context, pos)
                    {
                        context.level.set_block(pos, blocks::ICE);
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::BLOCKS;
    use clustine_world::Ticked;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn lava() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:lake_lava").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn a_lava_lake_in_stone_fills_its_lower_half_and_airs_and_ticks_its_upper() {
        let lake = lava();
        let mut world = World::flat(blocks::STONE, 100);
        let origin = BlockPos::new(8, 30, 8);
        assert!(world.with_context(17, |context| lake.place(context, origin)));
        let lava: Vec<_> = world
            .writes
            .iter()
            .filter(|(_, state)| *state == blocks::LAVA)
            .collect();
        let air: Vec<_> = world
            .writes
            .iter()
            .filter(|(_, state)| *state == blocks::CAVE_AIR)
            .collect();
        assert!(!lava.is_empty() && !air.is_empty());
        assert!(lava.iter().all(|(pos, _)| (26..30).contains(&pos.y)));
        assert!(air.iter().all(|(pos, _)| (30..34).contains(&pos.y)));
        for (pos, _) in &world.writes {
            assert!(
                (0..16).contains(&pos.x) && (0..16).contains(&pos.z),
                "{pos:?}"
            );
        }
        // One tick of cave air for each block of air, at once.
        assert_eq!(world.ticks.len(), air.len());
        for ((pos, _), (at, tick)) in air.iter().zip(&world.ticks) {
            assert_eq!(pos, at);
            let Ticked::Block(block) = tick.of else {
                panic!("{tick:?}");
            };
            assert_eq!(BLOCKS[usize::from(block)].name, "minecraft:cave_air");
            assert_eq!(tick.delay, 0);
        }
        // What stands above the air and the barrier is marked to take its shape.
        assert!(!world.marks.is_empty());
        assert!(world.marks.iter().all(|pos| (27..36).contains(&pos.y)));
    }

    #[test]
    fn a_lake_whose_air_would_touch_a_fluid_is_not_placed() {
        let lake = lava();
        let mut world = World::flat(blocks::WATER, 100);
        assert!(!world.with_context(17, |context| lake.place(context, BlockPos::new(8, 30, 8))));
        assert!(world.writes.is_empty() && world.ticks.is_empty());
    }

    #[test]
    fn a_lake_near_the_bottom_of_the_world_is_not_placed() {
        let lake = lava();
        let mut world = World::flat(blocks::STONE, 100);
        assert!(!world.with_context(17, |context| lake.place(context, BlockPos::new(8, -60, 8))));
    }

    #[test]
    fn the_edge_of_the_lake_is_beside_it_across_a_face() {
        let mut grid = Grid([false; 2048]);
        grid.0[Grid::index(5, 3, 5)] = true;
        assert!(grid.is_edge(6, 3, 5) && grid.is_edge(5, 4, 5) && grid.is_edge(5, 3, 4));
        assert!(!grid.is_edge(6, 4, 5) && !grid.is_edge(5, 3, 5));
    }
}
