// Adapted from SteelMC's `steel-utils/src/random/legacy_random.rs`.

//! `java.util.Random` as the game uses it (`LegacyRandomSource`): a linear
//! congruential generator with 48 bits of state.

use super::{NameHash, PositionalRandom, Random, RandomSource, RandomSplitter, gaussian, get_seed};

/// The multiplier of `java.util.Random`.
const MULTIPLIER: u64 = 0x0005_DEEC_E66D;
/// The increment of `java.util.Random`.
const INCREMENT: u64 = 0xB;
/// The state is 48 bits wide.
const MASK: u64 = 0xFFFF_FFFF_FFFF;

/// The generator the game had before 1.18 and still uses where old worlds must not
/// change: the blended terrain noise, the End's islands, the temperature noises of
/// biomes, carvers, and where structures are placed.
#[derive(Debug, Clone)]
pub struct LegacyRandom {
    seed: i64,
    next_gaussian: Option<f64>,
}

/// The factory of [`LegacyRandom`] generators (`LegacyPositionalRandomFactory`).
#[derive(Debug, Clone)]
pub struct LegacyRandomSplitter {
    seed: i64,
}

impl LegacyRandom {
    /// The generator for a seed, as `new LegacyRandomSource(seed)`.
    #[must_use]
    pub const fn from_seed(seed: u64) -> Self {
        Self {
            seed: ((seed ^ MULTIPLIER) & MASK) as i64,
            next_gaussian: None,
        }
    }

    /// The 48 bits of state, for tests and for saying where a generator stands.
    #[must_use]
    pub const fn state(&self) -> i64 {
        self.seed
    }

    /// Starts again from a seed, as `Random.setSeed`. A normally distributed number
    /// that was waiting is dropped.
    pub const fn set_seed(&mut self, seed: i64) {
        *self = Self::from_seed(seed as u64);
    }

    /// Seeds the generator for a chunk, as `WorldgenRandom.setLargeFeatureSeed`.
    /// Carvers are seeded so.
    pub fn set_large_feature_seed(&mut self, seed: i64, chunk_x: i32, chunk_z: i32) {
        self.set_seed(seed);
        let x_scale = self.next_i64();
        let z_scale = self.next_i64();
        self.set_seed(
            i64::from(chunk_x).wrapping_mul(x_scale)
                ^ i64::from(chunk_z).wrapping_mul(z_scale)
                ^ seed,
        );
    }

    /// Seeds the generator for a cell of a structure's grid and the structure's salt,
    /// as `WorldgenRandom.setLargeFeatureWithSalt`.
    pub const fn set_large_feature_with_salt(&mut self, seed: i64, x: i32, z: i32, salt: i32) {
        self.set_seed(
            (x as i64)
                .wrapping_mul(341_873_128_712)
                .wrapping_add((z as i64).wrapping_mul(132_897_987_541))
                .wrapping_add(seed)
                .wrapping_add(salt as i64),
        );
    }

    const fn next_bits(&mut self, bits: u32) -> i32 {
        let state = (self.seed as u64)
            .wrapping_mul(MULTIPLIER)
            .wrapping_add(INCREMENT)
            & MASK;
        self.seed = state as i64;
        (state >> (48 - bits)) as i32
    }

    /// Advances by `count` steps in a time that grows with the logarithm of `count`.
    ///
    /// A step is the map `s -> MULTIPLIER * s + INCREMENT` modulo 2^48. Two such maps
    /// compose to another of the same form, so the map of `count` steps is found by
    /// repeated squaring. The End's islands skip 17,292 numbers, and every octave a
    /// noise leaves out skips 262.
    const fn skip(&mut self, count: u64) {
        let mut total_multiplier: u64 = 1;
        let mut total_increment: u64 = 0;
        let mut multiplier = MULTIPLIER;
        let mut increment = INCREMENT;
        let mut remaining = count;
        while remaining > 0 {
            if remaining & 1 == 1 {
                total_increment = multiplier
                    .wrapping_mul(total_increment)
                    .wrapping_add(increment)
                    & MASK;
                total_multiplier = multiplier.wrapping_mul(total_multiplier) & MASK;
            }
            increment = multiplier.wrapping_mul(increment).wrapping_add(increment) & MASK;
            multiplier = multiplier.wrapping_mul(multiplier) & MASK;
            remaining >>= 1;
        }
        let state = self.seed as u64;
        self.seed = (total_multiplier
            .wrapping_mul(state)
            .wrapping_add(total_increment)
            & MASK) as i64;
    }
}

impl Random for LegacyRandom {
    fn fork(&mut self) -> Self {
        Self::from_seed(self.next_i64() as u64)
    }

    fn next_i32(&mut self) -> i32 {
        self.next_bits(32)
    }

    fn next_i32_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "the bound must be positive, as the game demands");
        if bound & (bound - 1) == 0 {
            // A power of two takes the high bits, which are the better ones.
            return ((i64::from(bound) * i64::from(self.next_bits(31))) >> 31) as i32;
        }
        loop {
            let sample = self.next_bits(31);
            let remainder = sample % bound;
            // Java lets this sum overflow to find the samples of the last, short run.
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
        // `BitRandomSource.nextDouble`: 53 bits from two draws, times the float
        // literal 1.110223E-16F widened to a double, which is exactly 2^-53.
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
        RandomSplitter::Legacy(LegacyRandomSplitter::new(self.next_i64()))
    }

    fn consume_count(&mut self, count: i32) {
        if count > 0 {
            self.skip(count as u64);
        }
    }
}

impl LegacyRandomSplitter {
    /// The factory with a given seed.
    #[must_use]
    pub const fn new(seed: i64) -> Self {
        Self { seed }
    }

    /// The factory's seed.
    #[must_use]
    pub const fn seed(&self) -> i64 {
        self.seed
    }
}

impl PositionalRandom for LegacyRandomSplitter {
    fn at(&self, x: i32, y: i32, z: i32) -> RandomSource {
        let position = get_seed(x, y, z);
        RandomSource::Legacy(LegacyRandom::from_seed((position ^ self.seed) as u64))
    }

    fn with_hash_of(&self, hash: &NameHash) -> RandomSource {
        RandomSource::Legacy(LegacyRandom::from_seed(
            (i64::from(hash.java_hash) ^ self.seed) as u64,
        ))
    }

    fn with_seed(&self, seed: u64) -> RandomSource {
        // The game's factory leaves its own seed out here.
        RandomSource::Legacy(LegacyRandom::from_seed(seed))
    }
}

#[cfg(test)]
mod tests {
    // The expected values are those of SteelMC's tests of the same file.

    use super::*;

    #[test]
    fn integers_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        let expected = [
            -1_155_484_576,
            -723_955_400,
            1_033_096_058,
            -1_690_734_402,
            -1_557_280_266,
            1_327_362_106,
            -1_930_858_313,
            502_539_523,
            -1_728_529_858,
            -938_301_587,
        ];
        for value in expected {
            assert_eq!(random.next_i32(), value);
        }
    }

    #[test]
    fn bounded_integers_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        for value in [0, 13, 4, 2, 5, 8, 11, 6, 9, 14] {
            assert_eq!(random.next_i32_bounded(0xf), value);
        }

        let mut random = LegacyRandom::from_seed(0);
        for _ in 0..10 {
            assert_eq!(random.next_i32_bounded(1), 0);
        }

        let mut random = LegacyRandom::from_seed(0);
        for value in [1, 1, 0, 1, 1, 0, 1, 0, 1, 1] {
            assert_eq!(random.next_i32_bounded(2), value);
        }
    }

    #[test]
    fn integers_between_two_bounds_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        for value in [1, 5, 2, 12, 12, 6, 12, 10, 4, 3] {
            assert_eq!(random.next_i32_between(1, 12), value);
        }

        let mut random = LegacyRandom::from_seed(0);
        for value in [1, 7, 9, 6, 7, 3, 3, 7, 3, 1] {
            assert_eq!(random.next_i32_between_exclusive(1, 12), value);
        }
    }

    #[test]
    fn doubles_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        let expected: [f64; 10] = [
            0.730_967_787_376_657,
            0.240_536_415_671_485_87,
            0.637_417_425_350_108_3,
            0.550_437_005_117_633_9,
            0.597_545_277_797_201_8,
            0.333_218_399_476_649_8,
            0.385_189_184_740_718_5,
            0.984_841_540_199_809,
            0.879_182_517_872_480_1,
            0.941_249_179_482_114_4,
        ];
        for value in expected {
            assert_eq!(random.next_f64().to_bits(), value.to_bits());
        }
    }

    #[test]
    fn floats_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        let expected: [f32; 10] = [
            0.730_967_76,
            0.831_441,
            0.240_536_39,
            0.606_345_2,
            0.637_417_4,
            0.309_050_56,
            0.550_437,
            0.117_006_6,
            0.597_545_27,
            0.781_534_6,
        ];
        for value in expected {
            assert_eq!(random.next_f32().to_bits(), value.to_bits());
        }
    }

    #[test]
    fn longs_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        let expected: [i64; 10] = [
            -4_962_768_465_676_381_896,
            4_437_113_781_045_784_766,
            -6_688_467_811_848_818_630,
            -8_292_973_307_042_192_125,
            -7_423_979_211_207_825_555,
            6_146_794_652_083_548_235,
            7_105_486_291_024_734_541,
            -279_624_296_851_435_688,
            -2_228_689_144_322_150_137,
            -1_083_761_183_081_836_303,
        ];
        for value in expected {
            assert_eq!(random.next_i64(), value);
        }
    }

    #[test]
    fn truth_values_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        let expected = [
            true, true, false, true, true, false, true, false, true, true,
        ];
        for value in expected {
            assert_eq!(random.next_bool(), value);
        }
    }

    #[test]
    fn normally_distributed_numbers_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        let expected: [f64; 10] = [
            0.802_533_063_739_030_5,
            -0.901_546_088_417_512_2,
            2.080_920_790_428_163,
            0.763_770_768_436_489_4,
            0.984_574_532_882_512_8,
            -1.683_412_258_767_342_8,
            -0.027_290_262_907_887_285,
            0.115_245_702_862_023_15,
            -0.390_167_041_379_937_74,
            -0.643_388_813_126_449,
        ];
        for value in expected {
            assert_eq!(random.next_gaussian().to_bits(), value.to_bits());
        }
    }

    #[test]
    fn triangles_are_javas() {
        let mut random = LegacyRandom::from_seed(0);
        let expected: [f64; 10] = [
            124.521_568_585_258_56,
            104.349_021_011_623_72,
            113.216_343_916_027_6,
            70.017_382_227_045_47,
            96.896_666_919_518_28,
            107.302_840_758_085_41,
            106.168_176_758_131_44,
            79.112_644_826_080_78,
            73.967_216_139_270_62,
            81.724_195_210_806_46,
        ];
        for value in expected {
            assert_eq!(random.triangle(100.0, 50.0).to_bits(), value.to_bits());
        }
    }

    #[test]
    fn skipping_is_the_same_as_drawing_and_discarding() {
        const COUNTS: &[i32] = &[0, 1, 2, 7, 31, 32, 33, 100, 262, 1023, 1024, 17292, 100_000];
        const SEEDS: &[u64] = &[0, 1, 0xDEAD_BEEF, 0x1234_5678_9ABC_DEF0];
        for &seed in SEEDS {
            for &count in COUNTS {
                let mut skipped = LegacyRandom::from_seed(seed);
                let mut drawn = LegacyRandom::from_seed(seed);
                skipped.consume_count(count);
                for _ in 0..count {
                    drawn.next_i32();
                }
                assert_eq!(
                    skipped.next_i64(),
                    drawn.next_i64(),
                    "seed {seed:#x}, count {count}"
                );
            }
        }
    }

    #[test]
    fn skipping_a_negative_count_does_nothing() {
        let mut skipped = LegacyRandom::from_seed(42);
        let mut untouched = LegacyRandom::from_seed(42);
        skipped.consume_count(-1);
        skipped.consume_count(i32::MIN);
        assert_eq!(skipped.next_i64(), untouched.next_i64());
    }

    #[test]
    fn forks_and_factories_are_javas() {
        let mut original = LegacyRandom::from_seed(0);
        let RandomSplitter::Legacy(splitter) = original.next_positional() else {
            panic!("a legacy generator made a xoroshiro factory");
        };
        assert_eq!(splitter.seed(), -4_962_768_465_676_381_896_i64);
        let mut by_name = splitter.with_hash_of(&NameHash::new("minecraft:offset"));
        assert_eq!(by_name.next_i32(), 103_436_829);

        let mut original = LegacyRandom::from_seed(0);
        let mut forked = original.fork();
        {
            let splitter = forked.next_positional();
            let mut by_name = splitter.with_hash_of(&NameHash::new("TEST STRING"));
            assert_eq!(by_name.next_i32(), -1_170_413_697);
            let mut by_number = splitter.with_seed(10);
            assert_eq!(by_number.next_i32(), -1_157_793_070);
            let mut by_position = splitter.at(1, 11, -111);
            assert_eq!(by_position.next_i32(), -1_213_890_343);
        }
        assert_eq!(original.next_i32(), 1_033_096_058);
        assert_eq!(forked.next_i32(), -888_301_832);
    }

    #[test]
    fn setting_the_seed_is_starting_from_that_seed() {
        let mut fresh = LegacyRandom::from_seed(12345);
        let mut reseeded = LegacyRandom::from_seed(0);
        let _ = reseeded.next_gaussian();
        reseeded.set_seed(12345);
        assert_eq!(reseeded.state(), fresh.state());
        for _ in 0..10 {
            assert_eq!(fresh.next_i64(), reseeded.next_i64());
        }
        assert_eq!(
            fresh.next_gaussian().to_bits(),
            reseeded.next_gaussian().to_bits()
        );
    }

    #[test]
    fn the_seed_of_a_structure_cell_is_the_games_sum() {
        let mut random = LegacyRandom::from_seed(0);
        random.set_large_feature_with_salt(0, 0, 0, 10_387_312);
        assert_eq!(random.state(), LegacyRandom::from_seed(10_387_312).state());

        random.set_large_feature_with_salt(123_456_789, 5, -3, 10_387_312);
        let sum: i64 =
            5_i64 * 341_873_128_712 + (-3_i64) * 132_897_987_541 + 123_456_789 + 10_387_312;
        assert_eq!(random.state(), LegacyRandom::from_seed(sum as u64).state());
    }

    #[test]
    fn the_seed_of_a_chunk_is_made_of_the_first_two_longs() {
        let x_scale = -4_962_768_465_676_381_896_i64;
        let z_scale = 4_437_113_781_045_784_766_i64;
        let expected = 3_i64.wrapping_mul(x_scale) ^ 5_i64.wrapping_mul(z_scale);

        let mut random = LegacyRandom::from_seed(0);
        random.set_large_feature_seed(0, 3, 5);
        assert_eq!(
            random.state(),
            LegacyRandom::from_seed(expected as u64).state()
        );
    }
}
