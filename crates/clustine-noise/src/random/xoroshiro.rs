// Adapted from SteelMC's `steel-utils/src/random/xoroshiro.rs`.

//! Xoroshiro128++, the generator of today's world generation
//! (`XoroshiroRandomSource` and `Xoroshiro128PlusPlus` in the game).

use super::{NameHash, PositionalRandom, Random, RandomSource, RandomSplitter, gaussian, get_seed};

const GOLDEN_RATIO_64: u64 = 0x9E37_79B9_7F4A_7C15;
const SILVER_RATIO_64: u64 = 0x6A09_E667_F3BC_C909;

/// The generator the overworld, the Nether and the End are made with: every noise of
/// the noise router, aquifers, ore veins and the surface take theirs from a factory of
/// this family, seeded with the world's seed.
#[derive(Debug, Clone)]
pub struct Xoroshiro {
    seed_lo: u64,
    seed_hi: u64,
    next_gaussian: Option<f64>,
}

/// The factory of [`Xoroshiro`] generators (`XoroshiroPositionalRandomFactory`).
#[derive(Debug, Clone)]
pub struct XoroshiroSplitter {
    seed_lo: u64,
    seed_hi: u64,
}

impl Xoroshiro {
    /// The generator for a seed, as `new XoroshiroRandomSource(seed)`: the seed is
    /// widened to 128 bits and both halves are mixed.
    #[must_use]
    pub const fn from_seed(seed: u64) -> Self {
        let (lo, hi) = Self::upgrade_seed_to_128_bit(seed);
        Self::new(mix_stafford_13(lo), mix_stafford_13(hi))
    }

    /// The generator for a seed whose halves are not mixed
    /// (`RandomSupport.upgradeSeedTo128bitUnmixed`).
    #[must_use]
    pub const fn from_seed_unmixed(seed: u64) -> Self {
        let (lo, hi) = Self::upgrade_seed_to_128_bit(seed);
        Self::new(lo, hi)
    }

    /// The generator in a given state. A state of all zeros would stay there for ever,
    /// so the game replaces it.
    #[must_use]
    pub const fn from_state(lo: u64, hi: u64) -> Self {
        Self::new(lo, hi)
    }

    const fn new(lo: u64, hi: u64) -> Self {
        let (lo, hi) = if (lo | hi) == 0 {
            (GOLDEN_RATIO_64, SILVER_RATIO_64)
        } else {
            (lo, hi)
        };
        Self {
            seed_lo: lo,
            seed_hi: hi,
            next_gaussian: None,
        }
    }

    const fn upgrade_seed_to_128_bit(seed: u64) -> (u64, u64) {
        let lo = seed ^ SILVER_RATIO_64;
        let hi = lo.wrapping_add(GOLDEN_RATIO_64);
        (lo, hi)
    }

    const fn next_bits(&mut self, bits: u32) -> u64 {
        self.next_random() >> (64 - bits)
    }

    const fn next_random(&mut self) -> u64 {
        let lo = self.seed_lo;
        let hi = self.seed_hi;
        let result = lo.wrapping_add(hi).rotate_left(17).wrapping_add(lo);
        let hi = hi ^ lo;
        self.seed_lo = lo.rotate_left(49) ^ hi ^ (hi << 21);
        self.seed_hi = hi.rotate_left(28);
        result
    }

    /// Starts again from a seed, as `XoroshiroRandomSource.setSeed`. A normally
    /// distributed number that was waiting is dropped.
    pub const fn set_seed(&mut self, seed: i64) {
        *self = Self::from_seed(seed as u64);
    }
}

const fn mix_stafford_13(z: u64) -> u64 {
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Random for Xoroshiro {
    fn fork(&mut self) -> Self {
        let lo = self.next_random();
        let hi = self.next_random();
        Self::new(lo, hi)
    }

    fn next_i32(&mut self) -> i32 {
        self.next_random() as i32
    }

    fn next_i32_bounded(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "the bound must be positive, as the game demands");
        let bound = bound as u32;
        // Lemire's method: the high half of a 32 by 32 bit product, with the few low
        // halves that would make it uneven drawn again.
        let mut product = u64::from(self.next_i32() as u32) * u64::from(bound);
        let mut low = product as u32;
        if low < bound {
            let threshold = bound.wrapping_neg() % bound;
            while low < threshold {
                product = u64::from(self.next_i32() as u32) * u64::from(bound);
                low = product as u32;
            }
        }
        (product >> 32) as i32
    }

    fn next_i64(&mut self) -> i64 {
        self.next_random() as i64
    }

    fn next_f32(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.960_464_5e-8
    }

    fn next_f64(&mut self) -> f64 {
        // The game's constant is the float literal 1.110223E-16F widened to a double.
        self.next_bits(53) as f64 * f64::from(1.110_223e-16_f32)
    }

    fn next_bool(&mut self) -> bool {
        (self.next_random() & 1) != 0
    }

    fn next_gaussian(&mut self) -> f64 {
        let mut stored = self.next_gaussian.take();
        let value = gaussian::next_gaussian(self, &mut stored);
        self.next_gaussian = stored;
        value
    }

    fn next_positional(&mut self) -> RandomSplitter {
        let seed_lo = self.next_random();
        let seed_hi = self.next_random();
        RandomSplitter::Xoroshiro(XoroshiroSplitter { seed_lo, seed_hi })
    }
}

impl XoroshiroSplitter {
    /// The factory with a given pair of seeds.
    #[must_use]
    pub const fn new(seed_lo: u64, seed_hi: u64) -> Self {
        Self { seed_lo, seed_hi }
    }
}

impl PositionalRandom for XoroshiroSplitter {
    fn at(&self, x: i32, y: i32, z: i32) -> RandomSource {
        let position = get_seed(x, y, z) as u64;
        RandomSource::Xoroshiro(Xoroshiro::new(position ^ self.seed_lo, self.seed_hi))
    }

    fn with_hash_of(&self, hash: &NameHash) -> RandomSource {
        let [lo, hi] = hash.md5;
        RandomSource::Xoroshiro(Xoroshiro::new(lo ^ self.seed_lo, hi ^ self.seed_hi))
    }

    fn with_seed(&self, seed: u64) -> RandomSource {
        RandomSource::Xoroshiro(Xoroshiro::new(seed ^ self.seed_lo, seed ^ self.seed_hi))
    }
}

#[cfg(test)]
mod tests {
    // The expected values are those of SteelMC's tests of the same file, which were
    // checked against the Java source.

    use super::*;

    fn xoroshiro(source: RandomSource) -> Xoroshiro {
        match source {
            RandomSource::Xoroshiro(random) => random,
            RandomSource::Legacy(_) => panic!("a xoroshiro factory made a legacy generator"),
        }
    }

    #[test]
    fn the_seed_mixer_gives_javas_values() {
        const CASES: &[(u64, i64)] = &[
            (0, 0),
            (1, 6238072747940578789),
            (64, -8456553050427055661),
            (4096, -1125827887270283392),
            (262144, -120227641678947436),
            (16777216, 6406066033425044679),
            (1073741824, 3143522559155490559),
            (16, -2773008118984693571),
            (1024, 8101005175654470197),
            (65536, -3551754741763842827),
            (4194304, -2737109459693184599),
            (2, -2606959012126976886),
            (128, -5825874238589581082),
            (8192, 1111983794319025228),
            (524288, -7964047577924347155),
            (33554432, -5634612006859462257),
            (2147483648, -1436547171018572641),
            (137438953472, -4514638798598940860),
            (8796093022208, -610572083552328405),
            (562949953421312, -263574021372026223),
            (36028797018963968, 7868130499179604987),
            (253, -4045451768301188906),
            (127, -6873224393826578139),
            (8447, 6670985465942597767),
            (524543, -6228499289678716485),
            (33554687, 2630391896919662492),
            (2147483903, -6879633228472053040),
            (137438953727, -5817997684975131823),
            (8796093022463, 2384436581894988729),
            (562949953421567, -5076179956679497213),
            (36028797018964223, -5993365784811617721),
        ];
        for &(input, expected) in CASES {
            assert_eq!(mix_stafford_13(input), expected as u64, "input {input}");
        }
    }

    #[test]
    fn integers_are_javas() {
        const EXPECTED: [i32; 10] = [
            -160476802,
            781697906,
            653572596,
            1337520923,
            -505875771,
            -47281585,
            342195906,
            1417498593,
            -1478887443,
            1560080270,
        ];
        let mut random = Xoroshiro::from_seed(0);
        for expected in EXPECTED {
            assert_eq!(random.next_i32(), expected);
        }
    }

    #[test]
    fn bounded_integers_are_javas() {
        const SMALL: [i32; 10] = [9, 1, 1, 3, 8, 9, 0, 3, 6, 3];
        const LARGE: [i32; 10] = [
            9784805, 470346, 13560642, 7320226, 14949645, 13460529, 2824352, 10938308, 14146127,
            4549185,
        ];
        let mut random = Xoroshiro::from_seed(0);
        for expected in SMALL {
            assert_eq!(random.next_i32_bounded(10), expected);
        }
        for expected in LARGE {
            assert_eq!(random.next_i32_bounded(0xFF_FFFF), expected);
        }
    }

    #[test]
    fn integers_between_two_bounds_are_javas() {
        const INCLUSIVE: [i32; 10] = [99, 59, 57, 65, 94, 100, 54, 66, 83, 68];
        const EXCLUSIVE: [i32; 10] = [98, 59, 57, 65, 94, 99, 53, 66, 82, 68];
        let mut random = Xoroshiro::from_seed(0);
        for expected in INCLUSIVE {
            assert_eq!(random.next_i32_between(50, 100), expected);
        }
        let mut random = Xoroshiro::from_seed(0);
        for expected in EXCLUSIVE {
            assert_eq!(random.next_i32_between_exclusive(50, 100), expected);
        }
    }

    #[test]
    fn doubles_are_javas() {
        const EXPECTED: [f64; 10] = [
            0.16474369376959186,
            0.7997457290026366,
            0.2511961888876212,
            0.11712489470639631,
            0.0997124786680137,
            0.7566797430601416,
            0.7723285712021574,
            0.9420469457586381,
            0.48056202536813664,
            0.6099690583914598,
        ];
        let mut random = Xoroshiro::from_seed(0);
        for expected in EXPECTED {
            assert_eq!(random.next_f64().to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn floats_are_javas() {
        const EXPECTED: [f32; 10] = [
            0.16474366,
            0.7997457,
            0.25119615,
            0.117124856,
            0.09971243,
            0.7566797,
            0.77232856,
            0.94204694,
            0.48056197,
            0.609969,
        ];
        let mut random = Xoroshiro::from_seed(0);
        for expected in EXPECTED {
            assert_eq!(random.next_f32().to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn longs_are_javas() {
        const EXPECTED: [i64; 10] = [
            3038984756725240190,
            -3694039286755638414,
            4633751808701151732,
            2160572957309072155,
            1839370574944072389,
            -4488466507718817201,
            -4199796579929588030,
            -1069045159880208415,
            8864804693509535725,
            -7194800960680693874,
        ];
        let mut random = Xoroshiro::from_seed(0);
        for expected in EXPECTED {
            assert_eq!(random.next_i64(), expected);
        }
    }

    #[test]
    fn truth_values_are_javas() {
        const EXPECTED: [bool; 10] = [
            false, false, false, true, true, true, false, true, true, false,
        ];
        let mut random = Xoroshiro::from_seed(0);
        for expected in EXPECTED {
            assert_eq!(random.next_bool(), expected);
        }
    }

    #[test]
    fn normally_distributed_numbers_are_javas() {
        const EXPECTED: [f64; 10] = [
            -0.48540690699780015,
            0.43399227545320296,
            -0.3283265251019599,
            -0.5052497078202575,
            -0.3772512828630807,
            0.2419080215945433,
            -0.42622066207565135,
            2.411315261138953,
            -1.1419147030553274,
            -0.05849758093810378,
        ];
        let mut random = Xoroshiro::from_seed(0);
        for expected in EXPECTED {
            assert_eq!(random.next_gaussian().to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn triangles_are_javas() {
        const EXPECTED: [f64; 10] = [
            6.824989823834776,
            10.670356470906125,
            6.71516367803936,
            9.151408127217596,
            9.352964834883384,
            8.291618967842293,
            8.954549938640508,
            11.833001837470519,
            10.65851306020791,
            11.684676364031647,
        ];
        let mut random = Xoroshiro::from_seed(0);
        for expected in EXPECTED {
            assert_eq!(random.triangle(10.0, 5.0).to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn a_fork_goes_its_own_way() {
        let mut random = Xoroshiro::from_seed(0);
        let mut forked = random.fork();
        assert_eq!(forked.next_i32(), 542195535);
        assert_eq!(random.next_i32(), 653572596);
    }

    #[test]
    fn the_factory_gives_javas_generators_by_name_number_and_position() {
        let mut random = Xoroshiro::from_seed(0);
        let mut forked = random.fork();
        assert_eq!(forked.next_i32(), 542195535);

        let splitter = forked.next_positional();
        let mut by_name = xoroshiro(splitter.with_hash_of(&NameHash::new("TEST STRING")));
        assert_eq!(by_name.next_i32(), -641435713);
        let mut by_number = xoroshiro(splitter.with_seed(42069));
        assert_eq!(by_number.next_i32(), -340700677);
        let mut by_position = xoroshiro(splitter.at(1337, 80085, -69420));
        assert_eq!(by_position.next_i32(), 790449132);

        assert_eq!(random.next_i32(), 653572596);
        assert_eq!(forked.next_i32(), 435917842);
    }

    #[test]
    fn a_state_of_zeros_is_replaced() {
        let mut random = Xoroshiro::from_state(0, 0);
        assert_eq!(random.next_i64(), 6807859099481836695);
    }

    #[test]
    fn setting_the_seed_drops_a_waiting_normal_number() {
        let mut random = Xoroshiro::from_seed(5);
        let _ = random.next_gaussian();
        random.set_seed(9);
        let mut fresh = Xoroshiro::from_seed(9);
        assert_eq!(
            random.next_gaussian().to_bits(),
            fresh.next_gaussian().to_bits()
        );
    }
}
