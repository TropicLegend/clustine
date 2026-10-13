// Adapted from SteelMC, steel-utils/src/climate/mod.rs and
// steel-worldgen/src/biomes/climate_sampler.rs at 885c4b3 (AGPL-3.0-or-later,
// Copyright (C) 2026 Alve Jeansson and contributors; see NOTICE). Changed for Clustine
// in October 2026: one sampler over any of the emitted routers in place of one for
// each dimension; the routers give single-precision values themselves.

//! The climate at a place: the six numbers the game looks a biome up by.
//!
//! Here a climate is the dimension's router asked at one position. The game of 26.3
//! does that when a single biome is wanted, but for the biomes of a chunk it has the
//! router fill a volume for each of the six (`MultiNoiseBiomeSource`'s resolver for a
//! chunk), and a router's volume is not always the same bits as its single positions
//! (`clustine-worldgen-data`, the notes on `interpolate`). For the six functions of
//! a climate it made no difference: `tests/java/GameBiomes.java` asked the game both
//! ways at every cell of 10,989 chunks of the three dimensions, and the six whole
//! numbers were the same at all 12,827,648 cells.

use clustine_worldgen_data::{NoiseRouter, RouterEntry};

/// The entries of a noise router that make up a climate, in the order of
/// [`Climate`]'s numbers.
pub const PARAMETERS: [RouterEntry; 6] = [
    RouterEntry::Temperature,
    RouterEntry::Vegetation,
    RouterEntry::Continents,
    RouterEntry::Erosion,
    RouterEntry::Depth,
    RouterEntry::Ridges,
];

/// The names the game gives the six, in the same order.
pub const PARAMETER_NAMES: [&str; 6] = [
    "temperature",
    "humidity",
    "continentalness",
    "erosion",
    "depth",
    "weirdness",
];

/// The place of depth among the six.
pub const DEPTH: usize = 4;

/// A climate: temperature, humidity, continentalness, erosion, depth and weirdness,
/// as the whole numbers the game compares, ten thousand to one unit of a noise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Climate(pub [i64; 6]);

/// A value of a density function as the whole number the game looks biomes up by:
/// times ten thousand in single precision, and what is behind the point dropped.
#[inline]
pub fn quantise(value: f32) -> i64 {
    (value * 10_000.0) as i64
}

impl Climate {
    /// The climate the dimension's router gives at a biome cell. `x`, `y` and `z`
    /// count cells of four blocks, and the functions are asked at the block with the
    /// lowest coordinates of the cell, as the game asks them.
    pub fn sample<R: NoiseRouter>(
        router: &R,
        memory: &mut R::Memory,
        x: i32,
        y: i32,
        z: i32,
    ) -> Climate {
        let (x, y, z) = (x << 2, y << 2, z << 2);
        Climate(PARAMETERS.map(|entry| quantise(router.sample(memory, entry, x, y, z))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_worldgen_data::{NetherRouter, OverworldRouter};

    #[test]
    fn a_value_is_quantised_to_ten_thousandths_towards_zero() {
        // The values of SteelMC's `test_quantize_coord` and
        // `test_target_point_from_floats`.
        assert_eq!(quantise(0.0), 0);
        assert_eq!(quantise(1.0), 10_000);
        assert_eq!(quantise(-1.0), -10_000);
        assert_eq!(quantise(0.5), 5_000);
        assert_eq!(quantise(-0.5), -5_000);
        assert_eq!(quantise(-0.3), -3_000);
        assert_eq!(quantise(0.1), 1_000);
        assert_eq!(quantise(0.2), 2_000);
        // Towards zero from both sides, and in single precision: 0.12345 is a little
        // above 1234.5 ten-thousandths as a `float`.
        assert_eq!(quantise(0.123_49), 1_234);
        assert_eq!(quantise(-0.123_49), -1_234);
    }

    #[test]
    fn a_climate_is_the_routers_six_entries_at_the_cells_lowest_block() {
        let router = OverworldRouter::new(13579);
        let mut memory = Default::default();
        let climate = Climate::sample(&router, &mut memory, -7, 19, 33);
        for (parameter, entry) in PARAMETERS.into_iter().enumerate() {
            let value = router.sample(&mut Default::default(), entry, -28, 76, 132);
            assert_eq!(climate.0[parameter], quantise(value), "{}", entry.name());
        }
    }

    #[test]
    fn only_depth_changes_with_height_in_the_overworld_and_nothing_in_the_nether() {
        // The filling of a chunk leans on this: it asks for the other five once a
        // column. It is so because the data scales the height of those noises by
        // nought; were the data to change, this is where it would show first.
        let overworld = OverworldRouter::new(13579);
        let nether = NetherRouter::new(13579);
        for (x, z) in [(0, 0), (-7, 33), (1201, -4455), (-73_075, 30_530)] {
            let low = Climate::sample(&overworld, &mut Default::default(), x, -16, z);
            let mut depths = vec![low.0[DEPTH]];
            for y in -15..80 {
                let at = Climate::sample(&overworld, &mut Default::default(), x, y, z);
                for parameter in (0..6).filter(|parameter| *parameter != DEPTH) {
                    assert_eq!(at.0[parameter], low.0[parameter]);
                }
                depths.push(at.0[DEPTH]);
            }
            assert!(depths.windows(2).all(|pair| pair[0] > pair[1]));

            let low = Climate::sample(&nether, &mut Default::default(), x, 0, z);
            for y in 1..64 {
                assert_eq!(
                    Climate::sample(&nether, &mut Default::default(), x, y, z),
                    low
                );
            }
        }
    }
}
