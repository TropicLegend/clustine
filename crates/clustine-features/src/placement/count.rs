// Adapted from SteelMC's `steel-core/src/worldgen/feature/{placed,placement,weather}.rs`.

//! The modifiers that repeat their position: `count`, `noise_based_count` and
//! `noise_threshold_count` give the position they are given a number of times; the
//! deprecated `count_on_every_layer` gives positions on every floor of a column.

use std::sync::LazyLock;

use clustine_data::{BlockState, blocks};
use clustine_noise::{LegacyRandom, PerlinSimplexNoise, Random};
use clustine_world::BlockPos;

use crate::blocks::same_block;
use crate::context::{Context, Heightmap};
use crate::placement::Modifier;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

/// The game's `Biome.BIOME_INFO_NOISE`: one octave of simplex noise from the fixed
/// seed 2345, the same in every world.
static BIOME_INFO_NOISE: LazyLock<PerlinSimplexNoise> =
    LazyLock::new(|| PerlinSimplexNoise::new(&mut LegacyRandom::from_seed(2345), &[0]));

/// The biome information noise at `x`, `z` (`getValue(x, z, false)`).
pub fn biome_info_noise(x: f64, z: f64) -> f64 {
    f64::from(BIOME_INFO_NOISE.get(x, z))
}

/// Gives `origin` `count` times.
fn repeat(origin: BlockPos, count: i32, out: &mut Vec<BlockPos>) {
    for _ in 0..count.max(0) {
        out.push(origin);
    }
}

/// `count`: as many times as its provider draws.
#[derive(Debug)]
pub struct Count(IntProvider);

impl Count {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self(IntProvider::read(&reader.field("count")?)?))
    }
}

impl Modifier for Count {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let count = self.0.sample(&mut *context.random);
        repeat(origin, count, out);
    }
}

/// `noise_based_count`: as many times as the biome information noise at the column
/// says, scaled.
#[derive(Debug)]
pub struct NoiseBasedCount {
    noise_to_count_ratio: i32,
    noise_factor: f64,
    noise_offset: f64,
}

impl NoiseBasedCount {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            noise_to_count_ratio: reader.field("noise_to_count_ratio")?.int()?,
            noise_factor: reader.field("noise_factor")?.double()?,
            noise_offset: match reader.optional("noise_offset") {
                Some(offset) => offset.double()?,
                None => 0.0,
            },
        })
    }

    fn count(&self, origin: BlockPos) -> i32 {
        let noise = biome_info_noise(
            f64::from(origin.x) / self.noise_factor,
            f64::from(origin.z) / self.noise_factor,
        );
        ((noise + self.noise_offset) * f64::from(self.noise_to_count_ratio)).ceil() as i32
    }
}

impl Modifier for NoiseBasedCount {
    fn positions(&self, _: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        repeat(origin, self.count(origin), out);
    }
}

/// `noise_threshold_count`: one count below a level of the biome information noise,
/// another above.
#[derive(Debug)]
pub struct NoiseThresholdCount {
    noise_level: f64,
    below_noise: i32,
    above_noise: i32,
}

impl NoiseThresholdCount {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            noise_level: reader.field("noise_level")?.double()?,
            below_noise: reader.field("below_noise")?.int()?,
            above_noise: reader.field("above_noise")?.int()?,
        })
    }
}

impl Modifier for NoiseThresholdCount {
    fn positions(&self, _: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let noise = biome_info_noise(f64::from(origin.x) / 200.0, f64::from(origin.z) / 200.0);
        let count = if noise < self.noise_level {
            self.below_noise
        } else {
            self.above_noise
        };
        repeat(origin, count, out);
    }
}

/// `count_on_every_layer`: for each floor of the columns, counting down from the
/// top, the count's number of random columns of the chunk, on that floor where the
/// column has one; until a floor where none had. Every position is found before any
/// is placed: the game collects them first.
#[derive(Debug)]
pub struct CountOnEveryLayer(IntProvider);

impl CountOnEveryLayer {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self(IntProvider::read(&reader.field("count")?)?))
    }
}

impl Modifier for CountOnEveryLayer {
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>) {
        let mut layer = 0;
        loop {
            let mut found_any = false;
            // The game draws the count again for every column it tries, in the
            // condition of its loop.
            let mut tried = 0;
            while tried < self.0.sample(&mut *context.random) {
                tried += 1;
                let x = context.random.next_i32_bounded(16) + origin.x;
                let z = context.random.next_i32_bounded(16) + origin.z;
                let start_y = context.level.height_at(Heightmap::MotionBlocking, x, z);
                if let Some(y) = on_ground(context, x, start_y, z, layer) {
                    out.push(BlockPos::new(x, y, z));
                    found_any = true;
                }
            }
            if !found_any {
                break;
            }
            layer += 1;
        }
    }
}

/// The y above the `layer`th floor of the column, counting down from `start_y`: a
/// block that is not empty under one that is, and not bedrock.
fn on_ground(
    context: &mut Context<'_, '_>,
    x: i32,
    start_y: i32,
    z: i32,
    layer: i32,
) -> Option<i32> {
    let mut current_layer = 0;
    let mut current = context.level.block(BlockPos::new(x, start_y, z));
    let mut y = start_y;
    while y > context.level.min_y() {
        let below = context.level.block(BlockPos::new(x, y - 1, z));
        if !is_empty(below) && is_empty(current) && !same_block(below, blocks::BEDROCK) {
            if current_layer == layer {
                return Some(y);
            }
            current_layer += 1;
        }
        current = below;
        y -= 1;
    }
    None
}

/// Air, water or lava, as the layers count them.
fn is_empty(state: BlockState) -> bool {
    state.props().is_air() || same_block(state, blocks::WATER) || same_block(state, blocks::LAVA)
}

#[cfg(test)]
mod tests {
    use clustine_noise::WorldgenRandom;

    use super::*;
    use crate::testing::World;

    #[test]
    fn a_count_gives_its_position_as_often_as_it_draws() {
        let count = Count(IntProvider::Uniform { min: 0, max: 5 });
        let mut world = World::flat(blocks::STONE, 0);
        let at = BlockPos::new(1, 2, 3);
        world.with_context(4, |context| {
            let mut expected = context.random.clone();
            let n = expected.next_i32_bounded(6);
            let mut out = Vec::new();
            count.positions(context, at, &mut out);
            assert_eq!(out, vec![at; n as usize]);
        });
    }

    #[test]
    fn the_biome_information_noise_is_the_game_s_single_octave() {
        // One octave at octave zero is the simplex noise itself, unscaled.
        let mut random = LegacyRandom::from_seed(2345);
        let octave = clustine_noise::SimplexNoise::new(&mut random);
        for (x, z) in [(0.0, 0.0), (1.5, -2.25), (100.0 / 80.0, -3000.0 / 80.0)] {
            assert_eq!(
                biome_info_noise(x, z).to_bits(),
                f64::from(octave.get_2d(x, z)).to_bits()
            );
        }
    }

    #[test]
    fn a_noise_based_count_rounds_up_its_scaled_noise() {
        let bamboo = NoiseBasedCount {
            noise_to_count_ratio: 160,
            noise_factor: 80.0,
            noise_offset: 0.3,
        };
        for x in [-400, 0, 37, 1200] {
            let at = BlockPos::new(x, 64, 2 * x);
            let noise = biome_info_noise(f64::from(x) / 80.0, f64::from(2 * x) / 80.0);
            assert_eq!(bamboo.count(at), ((noise + 0.3) * 160.0).ceil() as i32);
        }
    }

    #[test]
    fn a_threshold_count_takes_one_count_or_the_other() {
        let flowers = NoiseThresholdCount {
            noise_level: -0.8,
            below_noise: 15,
            above_noise: 4,
        };
        let mut world = World::flat(blocks::STONE, 0);
        for x in [0, 1000, -5000] {
            let at = BlockPos::new(x, 64, 0);
            let mut out = Vec::new();
            world.with_context(0, |context| flowers.positions(context, at, &mut out));
            let noise = biome_info_noise(f64::from(x) / 200.0, 0.0);
            assert_eq!(out.len(), if noise < -0.8 { 15 } else { 4 });
        }
    }

    #[test]
    fn every_layer_counts_floors_down_from_the_top_and_stops_at_an_empty_layer() {
        // Stone up to 10, a cave from 4 to 6: in each column two floors, at 11 and 4.
        let mut world = World::flat(blocks::STONE, 10);
        for x in -1..17 {
            for z in -1..17 {
                for y in 4..=6 {
                    world.set(BlockPos::new(x, y, z), blocks::AIR);
                }
            }
        }
        let layers = CountOnEveryLayer(IntProvider::Constant(2));
        let mut random = WorldgenRandom::from_seed(8);
        let mut expected = random.clone();
        let mut out = Vec::new();
        world.with_random(&mut random, None, |context| {
            layers.positions(context, BlockPos::new(0, 0, 0), &mut out);
        });
        // Two columns on each of two floors, then two columns that find no third.
        let mut want = Vec::new();
        for y in [11, 4] {
            for _ in 0..2 {
                let x = expected.next_i32_bounded(16);
                let z = expected.next_i32_bounded(16);
                want.push(BlockPos::new(x, y, z));
            }
        }
        assert_eq!(out, want);
    }
}
