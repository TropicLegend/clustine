// Adapted from SteelMC's `place_scattered_ore_feature` in
// `steel-core/src/worldgen/feature/features/ore.rs`.

//! `scattered_ore`: a few blocks of ore scattered near the origin, each tried once
//! (`ScatteredOreFeature`): the ancient debris.

use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::Feature;
use crate::features::frame::ore::OreConfiguration;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct ScatteredOre(OreConfiguration);

impl ScatteredOre {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self(OreConfiguration::read(reader)?))
    }
}

/// `Math.round` of the difference of two draws, scaled: away from the origin by up
/// to `distance` along one axis. Java rounds half up, exactly, which a sum in double
/// precision does too.
fn spread<R: Random + ?Sized>(random: &mut R, distance: i32) -> i32 {
    let value = (random.next_f32() - random.next_f32()) * distance as f32;
    (f64::from(value) + 0.5).floor() as i32
}

impl Feature for ScatteredOre {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let tries = context.random.next_i32_bounded(self.0.size + 1);
        for i in 0..tries {
            let distance = i.min(7);
            let x = spread(&mut *context.random, distance);
            let y = spread(&mut *context.random, distance);
            let z = spread(&mut *context.random, distance);
            let pos = BlockPos::new(origin.x + x, origin.y + y, origin.z + z);
            let state = context.level.block(pos);
            if let Some(ore) = self
                .0
                .replacement(context.level, &mut *context.random, state, pos)
            {
                context.level.set_block(pos, ore);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_noise::WorldgenRandom;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    #[test]
    fn java_rounds_halves_up_and_just_below_a_half_down() {
        assert_eq!((f64::from(0.5_f32) + 0.5).floor() as i32, 1);
        assert_eq!((f64::from(-0.5_f32) + 0.5).floor() as i32, 0);
        assert_eq!((f64::from(0.499_999_97_f32) + 0.5).floor() as i32, 0);
        assert_eq!((f64::from(-2.5_f32) + 0.5).floor() as i32, -2);
    }

    #[test]
    fn ancient_debris_is_scattered_in_netherrack_where_no_air_is_beside_it() {
        let (_, entry) = data::configured("minecraft:ore_ancient_debris_large").unwrap();
        let debris =
            crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap();
        let mut world = World::flat(blocks::NETHERRACK, 100);
        let origin = BlockPos::new(8, 20, 8);
        for seed in 0..20 {
            let mut random = WorldgenRandom::from_seed(seed);
            assert!(world.with_random(&mut random, None, |context| debris.place(context, origin)));
        }
        assert!(!world.writes.is_empty());
        for (pos, state) in &world.writes {
            assert_eq!(*state, blocks::ANCIENT_DEBRIS);
            assert!((pos.x - origin.x).abs() <= 2 && (pos.y - origin.y).abs() <= 2);
        }
    }

    #[test]
    fn the_first_try_is_the_origin_itself() {
        let config = OreConfiguration {
            targets: vec![(crate::predicates::RuleTest::AlwaysTrue, blocks::GOLD_ORE)],
            size: 1,
            discard_chance_on_air_exposure: 0.0,
        };
        let ore = ScatteredOre(config);
        let mut world = World::flat(blocks::STONE, 10);
        for seed in 0..10 {
            let mut random = WorldgenRandom::from_seed(seed);
            let mut expected = random.clone();
            let tries = expected.next_i32_bounded(2);
            world.writes.clear();
            world.with_random(&mut random, None, |context| {
                ore.place(context, BlockPos::new(1, 5, 1))
            });
            let placed: Vec<BlockPos> = world.writes.iter().map(|(pos, _)| *pos).collect();
            let want = if tries == 1 {
                vec![BlockPos::new(1, 5, 1)]
            } else {
                Vec::new()
            };
            assert_eq!(placed, want);
        }
    }
}
