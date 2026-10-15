// Adapted from SteelMC's `steel-core/src/worldgen/feature/{placed,predicates}.rs`.

//! The modifiers that pass their position on or drop it (`PlacementFilter`): `biome`,
//! `block_predicate_filter`, `random_chance`, `rarity_filter`,
//! `surface_relative_threshold_filter` and `surface_water_depth_filter`.

use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::context::{Context, Heightmap};
use crate::placement::Modifier;
use crate::predicates::BlockPredicate;
use crate::reader::{Read, Reader};

/// Gives `origin` where `passes`.
fn filter(passes: bool, origin: BlockPos, out: &mut Vec<BlockPos>) {
    if passes {
        out.push(origin);
    }
}

/// `biome`: the position only where its biome lists the placed feature the run is
/// placing.
#[derive(Debug)]
pub struct BiomeFilter;

impl Modifier for BiomeFilter {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        // The game throws here for a placed feature that another places: it has no
        // top feature to ask about. A run's attempt keeps nothing of a feature that
        // throws, as the game's crash would.
        let top = context.top.expect(
            "tried to biome check an unregistered feature, or a feature that should not restrict the biome",
        );
        let biome = context.level.biome(origin);
        filter(context.settings.lists(biome.0, top), origin, out);
    }
}

/// `block_predicate_filter`: the position where the predicate holds there.
#[derive(Debug)]
pub struct BlockPredicateFilter(BlockPredicate);

impl BlockPredicateFilter {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self(BlockPredicate::read(&reader.field("predicate")?)?))
    }
}

impl Modifier for BlockPredicateFilter {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let passes = self.0.test(context, origin);
        filter(passes, origin, out);
    }
}

/// `random_chance`: the position with a chance.
#[derive(Debug)]
pub struct RandomChance(f32);

impl RandomChance {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self(reader.field("chance")?.float()?))
    }
}

impl Modifier for RandomChance {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let passes = context.random.next_f32() < self.0;
        filter(passes, origin, out);
    }
}

/// `rarity_filter`: the position once in `chance` times.
#[derive(Debug)]
pub struct RarityFilter(i32);

impl RarityFilter {
    pub fn read(reader: &Reader) -> Read<Self> {
        let chance = reader.field("chance")?;
        let value = chance.int()?;
        if value <= 0 {
            return Err(chance.error("a rarity is at least one"));
        }
        Ok(Self(value))
    }
}

impl Modifier for RarityFilter {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let passes = context.random.next_f32() < 1.0 / self.0 as f32;
        filter(passes, origin, out);
    }
}

/// `surface_relative_threshold_filter`: the position where its y lies within a range
/// relative to a heightmap's surface there.
#[derive(Debug)]
pub struct SurfaceRelativeThresholdFilter {
    heightmap: Heightmap,
    min: i32,
    max: i32,
}

impl SurfaceRelativeThresholdFilter {
    pub fn read(reader: &Reader) -> Read<Self> {
        let heightmap = reader.field("heightmap")?;
        let name = heightmap.text()?;
        Ok(Self {
            heightmap: Heightmap::named(name)
                .ok_or_else(|| heightmap.error(format!("{name} is not a heightmap")))?,
            min: match reader.optional("min_inclusive") {
                Some(min) => min.int()?,
                None => i32::MIN,
            },
            max: match reader.optional("max_inclusive") {
                Some(max) => max.int()?,
                None => i32::MAX,
            },
        })
    }
}

impl Modifier for SurfaceRelativeThresholdFilter {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        // In `long`, as the game sums them, so the open ends do not overflow.
        let surface = i64::from(context.level.height_at(self.heightmap, origin.x, origin.z));
        let y = i64::from(origin.y);
        let passes = surface + i64::from(self.min) <= y && y <= surface + i64::from(self.max);
        filter(passes, origin, out);
    }
}

/// `surface_water_depth_filter`: the position where the water above the ocean floor
/// is no deeper than a depth.
#[derive(Debug)]
pub struct SurfaceWaterDepthFilter(i32);

impl SurfaceWaterDepthFilter {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self(reader.field("max_water_depth")?.int()?))
    }
}

impl Modifier for SurfaceWaterDepthFilter {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let floor = context
            .level
            .height_at(Heightmap::OceanFloor, origin.x, origin.z);
        let surface = context
            .level
            .height_at(Heightmap::WorldSurface, origin.x, origin.z);
        filter(surface - floor <= self.0, origin, out);
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_noise::WorldgenRandom;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn run(world: &mut World, modifier: &dyn Modifier, seed: u64, at: BlockPos) -> Vec<BlockPos> {
        let mut out = Vec::new();
        world.with_context(seed, |context| modifier.positions(context, at, &mut out));
        out
    }

    #[test]
    fn a_rarity_filter_passes_one_in_its_chance() {
        let rarity = RarityFilter(7);
        let mut world = World::flat(blocks::STONE, 0);
        for seed in 0..40 {
            let mut expected = WorldgenRandom::from_seed(seed);
            let passes = expected.next_f32() < 1.0 / 7.0;
            let at = BlockPos::new(0, 0, 0);
            assert_eq!(
                run(&mut world, &rarity, seed, at).len(),
                usize::from(passes)
            );
        }
    }

    #[test]
    fn a_random_chance_draws_one_float() {
        let chance = RandomChance(0.2);
        let mut world = World::flat(blocks::STONE, 0);
        for seed in 0..40 {
            let mut expected = WorldgenRandom::from_seed(seed);
            let passes = expected.next_f32() < 0.2;
            assert_eq!(
                run(&mut world, &chance, seed, BlockPos::new(1, 1, 1)).len(),
                usize::from(passes)
            );
        }
    }

    #[test]
    fn the_water_depth_is_the_world_surface_above_the_ocean_floor() {
        let shallow = SurfaceWaterDepthFilter(2);
        let mut world = World::flat(blocks::SAND, 60);
        for y in 61..=62 {
            world.set(BlockPos::new(3, y, 3), blocks::WATER);
        }
        for y in 61..=63 {
            world.set(BlockPos::new(4, y, 4), blocks::WATER);
        }
        assert_eq!(
            run(&mut world, &shallow, 0, BlockPos::new(3, 70, 3)).len(),
            1
        );
        assert_eq!(
            run(&mut world, &shallow, 0, BlockPos::new(4, 70, 4)).len(),
            0
        );
    }

    #[test]
    fn a_threshold_relative_to_the_surface_has_open_ends() {
        let deep = SurfaceRelativeThresholdFilter {
            heightmap: Heightmap::OceanFloorWg,
            min: i32::MIN,
            max: -13,
        };
        let mut world = World::flat(blocks::STONE, 63);
        assert_eq!(run(&mut world, &deep, 0, BlockPos::new(0, 51, 0)).len(), 1);
        assert_eq!(run(&mut world, &deep, 0, BlockPos::new(0, 52, 0)).len(), 0);
        assert_eq!(run(&mut world, &deep, 0, BlockPos::new(0, -64, 0)).len(), 1);
    }

    #[test]
    fn the_biome_filter_asks_whether_the_biome_lists_the_top_feature() {
        let settings = crate::testing::settings();
        let plains = data::biome_features()
            .iter()
            .position(|biome| biome.biome == "minecraft:plains")
            .unwrap() as u16;
        let ocean = data::biome_features()
            .iter()
            .position(|biome| biome.biome == "minecraft:deep_ocean")
            .unwrap() as u16;
        let (flowers, _) = data::placed("minecraft:flower_plains").unwrap();
        assert!(settings.lists(plains, flowers));
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        let at = BlockPos::new(0, 65, 0);
        for (biome, passes) in [(plains, true), (ocean, false)] {
            world.biome = clustine_world::Biome(biome);
            let mut random = WorldgenRandom::from_seed(0);
            let mut out = Vec::new();
            world.with_random(&mut random, Some(flowers), |context| {
                BiomeFilter.positions(context, at, &mut out);
            });
            assert_eq!(out.len(), usize::from(passes));
        }
    }

    #[test]
    #[should_panic(expected = "biome check")]
    fn the_biome_filter_of_a_feature_another_places_fails_as_the_game_s_does() {
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        run(&mut world, &BiomeFilter, 0, BlockPos::new(0, 0, 0));
    }
}
