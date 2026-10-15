// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/block_pile.rs`.

//! `block_pile`: a loose heap of blocks two high on sturdy ground (`BlockPileFeature`):
//! hay, melons, pumpkins, snow and ice in villages and on the ground.

use clustine_data::{Face, SupportType, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::same_block;
use crate::context::Context;
use crate::feature::Feature;
use crate::features::ground::shared::between_closed;
use crate::providers::Provider;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct BlockPile {
    state_provider: Provider,
}

impl BlockPile {
    pub fn read(reader: &Reader) -> Read<Self> {
        let pile = Self {
            state_provider: Provider::read(&reader.field("state_provider")?)?,
        };
        reader.finish()?;
        Ok(pile)
    }

    /// `tryPlaceBlock`: on air over ground that carries it.
    fn try_place(&self, context: &mut Context<'_, '_>, pos: BlockPos) {
        if !context.level.block(pos).props().is_air() || !may_place_on(context, pos) {
            return;
        }
        let state = self.state_provider.state(context, pos);
        context.level.set_block(pos, state);
    }
}

/// `mayPlaceOn`: a dirt path carries a block every other time, by a draw; anything
/// else when its top is sturdy.
fn may_place_on(context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
    let below = context.level.block(pos.offset(0, -1, 0));
    if same_block(below, blocks::DIRT_PATH) {
        return context.random.next_bool();
    }
    below.props().is_face_sturdy(Face::Up, SupportType::Full)
}

impl Feature for BlockPile {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if origin.y < context.level.min_y() + 5 {
            return false;
        }
        let xr = 2 + context.random.next_i32_bounded(2);
        let zr = 2 + context.random.next_i32_bounded(2);
        for pos in between_closed(origin.offset(-xr, 0, -zr), origin.offset(xr, 1, zr)) {
            let dx = origin.x - pos.x;
            let dz = origin.z - pos.z;
            let reach = context.random.next_f32() * 10.0 - context.random.next_f32() * 6.0;
            if (dx * dx + dz * dz) as f32 <= reach {
                self.try_place(context, pos);
            // The game compares the float with the double 0.031.
            } else if f64::from(context.random.next_f32()) < 0.031 {
                self.try_place(context, pos);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn named(name: &str) -> Box<dyn Feature> {
        let (_, entry) = data::configured(name).unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn a_pile_of_hay_lies_on_the_ground_and_one_above_it() {
        let pile = named("minecraft:pile_hay");
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        assert!(world.with_context(21, |context| pile.place(context, BlockPos::new(8, 65, 8))));
        assert!(!world.writes.is_empty());
        for (pos, state) in &world.writes {
            assert!(same_block(*state, blocks::HAY_BLOCK));
            assert!((65..=66).contains(&pos.y), "{pos:?}");
            assert!((pos.x - 8).abs() <= 3 && (pos.z - 8).abs() <= 3, "{pos:?}");
        }
    }

    #[test]
    fn a_pile_on_a_dirt_path_draws_for_each_block_it_would_place() {
        let pile = named("minecraft:pile_snow");
        let mut world = World::flat(blocks::DIRT_PATH, 64);
        world.with_context(4, |context| pile.place(context, BlockPos::new(8, 65, 8)));
        // Snow layers lie only on the path, by a coin's toss each.
        assert!(world.writes.iter().all(|(pos, _)| pos.y == 65));
    }

    #[test]
    fn a_pile_near_the_bottom_of_the_world_is_not_placed() {
        let pile = named("minecraft:pile_hay");
        let mut world = World::flat(blocks::STONE, -62);
        assert!(!world.with_context(1, |context| pile.place(context, BlockPos::new(8, -61, 8))));
    }
}
