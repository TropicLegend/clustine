// Adapted from SteelMC's `steel-utils/src/random/worldgen_random.rs`.

//! The generator features are placed with (`WorldgenRandom` around a
//! `XoroshiroRandomSource`).

use super::{Random, RandomSplitter, Xoroshiro, gaussian};

/// The generator of a chunk's decoration: trees, ores, flowers, lakes.
///
/// The game wraps a xoroshiro generator in a class that descends from the legacy one,
/// so every number is cut from the wrapped generator's longs by the legacy rules
/// (`BitRandomSource`). Its integers, doubles and normal numbers are therefore not
/// those of a plain [`Xoroshiro`] with the same seed.
#[derive(Debug, Clone)]
pub struct WorldgenRandom {
    source: Xoroshiro,
    next_gaussian: Option<f64>,
}

impl WorldgenRandom {
    /// The generator for a seed.
    #[must_use]
    pub const fn from_seed(seed: u64) -> Self {
        Self {
            source: Xoroshiro::from_seed(seed),
            next_gaussian: None,
        }
    }

    /// Starts the wrapped generator again from a seed.
    ///
    /// A normally distributed number that was waiting stays: the game's class
    /// inherits that slot from the legacy generator and its `setSeed` only reaches
    /// the wrapped one. So the first normal number of a feature can be one left over
    /// from the feature before it.
    pub const fn set_seed(&mut self, seed: i64) {
        self.source.set_seed(seed);
    }

    /// Seeds the generator for the decoration of a chunk, given the world's seed and
    /// the chunk's lowest block corner, and returns the seed it settled on
    /// (`WorldgenRandom.setDecorationSeed`).
    pub fn set_decoration_seed(&mut self, seed: i64, block_x: i32, block_z: i32) -> i64 {
        self.set_seed(seed);
        let x_scale = self.next_i64() | 1;
        let z_scale = self.next_i64() | 1;
        let decoration_seed = i64::from(block_x)
            .wrapping_mul(x_scale)
            .wrapping_add(i64::from(block_z).wrapping_mul(z_scale))
            ^ seed;
        self.set_seed(decoration_seed);
        decoration_seed
    }

    /// Seeds the generator for one feature of a chunk, given the chunk's decoration
    /// seed, the feature's index in its step and the step
    /// (`WorldgenRandom.setFeatureSeed`).
    pub const fn set_feature_seed(&mut self, decoration_seed: i64, feature_index: i32, step: i32) {
        let feature_seed = decoration_seed
            .wrapping_add(feature_index as i64)
            .wrapping_add(10_000_i64.wrapping_mul(step as i64));
        self.set_seed(feature_seed);
    }

    fn next_bits(&mut self, bits: u32) -> i32 {
        ((self.source.next_i64() as u64) >> (64 - bits)) as i32
    }
}

impl Random for WorldgenRandom {
    fn fork(&mut self) -> Self {
        Self {
            source: self.source.fork(),
            next_gaussian: None,
        }
    }

    fn next_i32(&mut self) -> i32 {
        self.next_bits(32)
    }

    fn next_i32_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "the bound must be positive, as the game demands");
        if bound & (bound - 1) == 0 {
            return ((i64::from(bound) * i64::from(self.next_bits(31))) >> 31) as i32;
        }
        loop {
            let sample = self.next_bits(31);
            let remainder = sample % bound;
            if sample.wrapping_sub(remainder).wrapping_add(bound - 1) >= 0 {
                return remainder;
            }
        }
    }

    fn next_i64(&mut self) -> i64 {
        let upper = self.next_i32();
        let lower = self.next_i32();
        (i64::from(upper) << 32).wrapping_add(i64::from(lower))
    }

    fn next_f32(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.960_464_5e-8_f32
    }

    fn next_f64(&mut self) -> f64 {
        let upper = i64::from(self.next_bits(26));
        let lower = i64::from(self.next_bits(27));
        ((upper << 27) + lower) as f64 * f64::from(1.110_223e-16_f32)
    }

    fn next_bool(&mut self) -> bool {
        self.next_bits(1) != 0
    }

    fn next_gaussian(&mut self) -> f64 {
        let mut stored = self.next_gaussian.take();
        let value = gaussian::next_gaussian(self, &mut stored);
        self.next_gaussian = stored;
        value
    }

    fn next_positional(&mut self) -> RandomSplitter {
        self.source.next_positional()
    }
}

#[cfg(test)]
mod tests {
    // The expected values are those of SteelMC's tests of the same file, which are
    // from a trace of the game.

    use super::*;

    #[test]
    fn the_decoration_seed_of_a_chunk_is_the_games() {
        let mut random = WorldgenRandom::from_seed(0);
        assert_eq!(
            random.set_decoration_seed(13_579, -6_695_392, 5_868_656),
            7_632_291_757_650_236_667,
        );
    }

    #[test]
    fn the_first_vein_of_dirt_of_a_chunk_starts_where_the_game_starts_it() {
        let mut random = WorldgenRandom::from_seed(0);
        let decoration_seed = random.set_decoration_seed(13_579, -6_695_392, 5_868_656);
        random.set_feature_seed(decoration_seed, 0, 6);

        let x = -6_695_392 + random.next_i32_bounded(16);
        let z = 5_868_656 + random.next_i32_bounded(16);
        let y = random.next_i32_bounded(161);
        assert_eq!((x, y, z), (-6_695_386, 149, 5_868_662));
    }

    #[test]
    fn a_waiting_normal_number_outlives_the_seed_of_the_next_feature() {
        let mut random = WorldgenRandom::from_seed(123);
        let _ = random.next_gaussian();
        random.set_feature_seed(456, 7, 8);

        let mut kept = WorldgenRandom::from_seed(123);
        let _ = kept.next_gaussian();
        assert_eq!(
            random.next_gaussian().to_bits(),
            kept.next_gaussian().to_bits()
        );

        let mut reseeded = WorldgenRandom::from_seed(0);
        reseeded.set_feature_seed(456, 7, 8);
        assert_eq!(
            random.next_gaussian().to_bits(),
            reseeded.next_gaussian().to_bits()
        );
    }
}
