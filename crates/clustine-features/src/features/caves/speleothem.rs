// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/speleothem.rs`.

//! `speleothem`: one pointed dripstone or sulfur spike, one or two blocks long, hanging
//! from a ceiling or standing on a floor, with a patch of its base block spread around
//! its root (`SpeleothemFeature`, 26.x's `PointedDripstoneFeature` made general). The
//! data places it through `simple_random_selector`s that scan down or up for the floor
//! or the ceiling first (`pointed_dripstone`, `sulfur_spike`).

use clustine_data::Face;
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::shared::{SpeleothemBlocks, is_empty_or_water, random_direction};
use crate::blocks::{HORIZONTAL, relative};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct Speleothem {
    blocks: SpeleothemBlocks,
    chance_of_taller_generation: f32,
    chance_of_directional_spread: f32,
    chance_of_spread_radius2: f32,
    chance_of_spread_radius3: f32,
}

impl Speleothem {
    pub fn read(reader: &Reader) -> Read<Self> {
        let chance = |key: &str, default: f32| -> Read<f32> {
            match reader.optional(key) {
                Some(value) => value.float(),
                None => Ok(default),
            }
        };
        let speleothem = Self {
            blocks: SpeleothemBlocks::read(reader)?,
            chance_of_taller_generation: chance("chance_of_taller_generation", 0.2)?,
            chance_of_directional_spread: chance("chance_of_directional_spread", 0.7)?,
            chance_of_spread_radius2: chance("chance_of_spread_radius2", 0.5)?,
            chance_of_spread_radius3: chance("chance_of_spread_radius3", 0.5)?,
        };
        reader.finish()?;
        Ok(speleothem)
    }

    /// `getTipDirection`: down from a base above, up from a base below, a coin's
    /// choice where both are bases.
    fn tip_direction<R: Random + ?Sized>(
        &self,
        level: &mut Level<'_>,
        random: &mut R,
        pos: BlockPos,
    ) -> Option<Face> {
        let above = self.blocks.is_base(level.block(pos.offset(0, 1, 0)));
        let below = self.blocks.is_base(level.block(pos.offset(0, -1, 0)));
        match (above, below) {
            (true, true) => Some(if random.next_bool() {
                Face::Down
            } else {
                Face::Up
            }),
            (true, false) => Some(Face::Down),
            (false, true) => Some(Face::Up),
            (false, false) => None,
        }
    }

    /// `createPatchOfBaseBlocks`: the root, then up to three steps out from it in each
    /// horizontal direction, each step less likely than the one before.
    fn create_patch(&self, context: &mut Context<'_, '_>, pos: BlockPos) {
        self.blocks.place_base_if_possible(context.level, pos);
        for direction in HORIZONTAL {
            if context.random.next_f32() > self.chance_of_directional_spread {
                continue;
            }
            let one = relative(pos, direction);
            self.blocks.place_base_if_possible(context.level, one);
            if context.random.next_f32() > self.chance_of_spread_radius2 {
                continue;
            }
            let two = relative(one, random_direction(&mut *context.random));
            self.blocks.place_base_if_possible(context.level, two);
            if context.random.next_f32() > self.chance_of_spread_radius3 {
                continue;
            }
            let three = relative(two, random_direction(&mut *context.random));
            self.blocks.place_base_if_possible(context.level, three);
        }
    }
}

impl Feature for Speleothem {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let Some(tip) = self.tip_direction(context.level, &mut *context.random, origin) else {
            return false;
        };
        self.create_patch(context, relative(origin, tip.opposite()));
        let taller = context.random.next_f32() < self.chance_of_taller_generation
            && is_empty_or_water(context.level.block(relative(origin, tip)));
        let height = if taller { 2 } else { 1 };
        self.blocks.grow(context.level, origin, tip, height, false);
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    /// The speleothem the data's `pointed_dripstone` places first, read from the data.
    fn pointed_dripstone() -> Speleothem {
        let (_, entry) = data::configured("minecraft:pointed_dripstone").unwrap();
        let features = entry.data.get("features").unwrap();
        let clustine_worldgen_data::feature::Data::List(features) = features else {
            panic!("a list");
        };
        let feature = features[0].get("feature").unwrap();
        let reader = Reader::new(feature, "test");
        reader.kind().unwrap();
        Speleothem::read(&reader).unwrap()
    }

    #[test]
    fn the_data_reads_with_the_games_default_chances() {
        let speleothem = pointed_dripstone();
        assert_eq!(speleothem.chance_of_taller_generation, 0.2);
        assert_eq!(speleothem.chance_of_directional_spread, 0.7);
        assert_eq!(speleothem.blocks.base, blocks::DRIPSTONE_BLOCK);
    }

    #[test]
    fn a_stalagmite_stands_on_stone_with_dripstone_spread_under_it() {
        let speleothem = pointed_dripstone();
        let mut world = World::flat(blocks::STONE, 63);
        let mut placed = 0;
        for seed in 0..8 {
            let mut world = World::flat(blocks::STONE, 63);
            if world.with_context(seed, |context| {
                speleothem.place(context, BlockPos::new(4, 64, 4))
            }) {
                placed += 1;
            }
            let tip = world.get(BlockPos::new(4, 64, 4));
            assert_eq!(tip.block_id(), blocks::POINTED_DRIPSTONE.block_id());
            assert_eq!(tip.property("vertical_direction"), Some("up"));
            assert_eq!(world.get(BlockPos::new(4, 63, 4)), blocks::DRIPSTONE_BLOCK);
        }
        assert_eq!(placed, 8);
        // Over air there is nothing to grow from.
        assert!(!world.with_context(1, |context| {
            speleothem.place(context, BlockPos::new(4, 80, 4))
        }));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_stalactite_hangs_from_a_ceiling() {
        let speleothem = pointed_dripstone();
        let mut world = World::flat(blocks::STONE, 70);
        for y in 60..=65 {
            world.set(BlockPos::new(4, y, 4), blocks::AIR);
        }
        assert!(world.with_context(3, |context| {
            speleothem.place(context, BlockPos::new(4, 65, 4))
        }));
        let tip = world.get(BlockPos::new(4, 65, 4));
        assert_eq!(tip.property("vertical_direction"), Some("down"));
        assert_eq!(world.get(BlockPos::new(4, 66, 4)), blocks::DRIPSTONE_BLOCK);
    }
}
