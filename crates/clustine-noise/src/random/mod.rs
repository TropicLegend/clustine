// Adapted from SteelMC's `steel-utils/src/random/mod.rs`.

//! The game's random number generators.
//!
//! World generation never asks one generator for everything. It makes a generator from
//! the world's seed, and from that a [`RandomSplitter`]: a factory that gives a fresh
//! generator for a name ("minecraft:temperature", "octave_-7") or for a block position.
//! That is what makes a noise or an ore vein the same whichever chunk is made first.
//!
//! There are two families. [`Xoroshiro`] is what the overworld, the Nether and the End
//! use today. [`LegacyRandom`] is `java.util.Random`, which the game still uses for the
//! blended terrain noise, the End's islands, carvers and the placement of structures.
//! [`WorldgenRandom`] is the generator features are placed with.

mod gaussian;
mod legacy;
mod name_hash;
mod worldgen_random;
mod xoroshiro;

pub use legacy::{LegacyRandom, LegacyRandomSplitter};
pub use name_hash::NameHash;
pub use worldgen_random::WorldgenRandom;
pub use xoroshiro::{Xoroshiro, XoroshiroSplitter};

/// A generator of random numbers, as the game's `RandomSource`.
///
/// The provided methods are the game's own definitions in terms of the others, so an
/// implementation only has to say where its bits come from.
pub trait Random {
    /// A new generator seeded from this one, which advances.
    #[must_use]
    fn fork(&mut self) -> Self
    where
        Self: Sized;

    /// Any `i32`.
    fn next_i32(&mut self) -> i32;

    /// An `i32` from zero up to but not including `bound`, which must be positive.
    fn next_i32_bounded(&mut self, bound: i32) -> i32;

    /// An `i32` from `min` to `max`, both included.
    fn next_i32_between(&mut self, min: i32, max: i32) -> i32 {
        self.next_i32_bounded(max - min + 1) + min
    }

    /// An `i32` from `min` up to but not including `max`.
    fn next_i32_between_exclusive(&mut self, min: i32, max: i32) -> i32 {
        min + self.next_i32_bounded(max - min)
    }

    /// Any `i64`.
    fn next_i64(&mut self) -> i64;

    /// An `f32` from zero up to but not including one.
    fn next_f32(&mut self) -> f32;

    /// An `f64` from zero up to but not including one.
    fn next_f64(&mut self) -> f64;

    /// Either truth value.
    fn next_bool(&mut self) -> bool;

    /// A normally distributed `f64` with mean zero and deviation one.
    fn next_gaussian(&mut self) -> f64;

    /// The game's `triangle`: the difference of two uniform numbers, scaled by `max`
    /// and moved to `min`. The names are the game's; `min` is the centre.
    fn triangle(&mut self, min: f64, max: f64) -> f64 {
        min + max * (self.next_f64() - self.next_f64())
    }

    /// [`Random::triangle`] in single precision.
    fn triangle_f32(&mut self, min: f32, max: f32) -> f32 {
        min + max * (self.next_f32() - self.next_f32())
    }

    /// A factory of generators by name and by position, seeded from this generator,
    /// which advances.
    fn next_positional(&mut self) -> RandomSplitter;

    /// Advances as `count` calls of [`Random::next_i32`] would. The game does this
    /// where an octave of a noise is left out, so that the octaves after it are the
    /// same with and without it.
    fn consume_count(&mut self, count: i32) {
        for _ in 0..count {
            self.next_i32();
        }
    }
}

/// A factory of generators, as the game's `PositionalRandomFactory`. The same question
/// always gets a generator in the same state.
pub trait PositionalRandom {
    /// The generator for a block position.
    fn at(&self, x: i32, y: i32, z: i32) -> RandomSource;

    /// The generator for a name, such as a noise's identifier.
    fn with_hash_of(&self, hash: &NameHash) -> RandomSource;

    /// The generator for a number.
    fn with_seed(&self, seed: u64) -> RandomSource;
}

/// Either family of generator, for code that is told by the world's settings which one
/// to use.
#[derive(Debug, Clone)]
pub enum RandomSource {
    /// The generator of today's dimensions.
    Xoroshiro(Xoroshiro),
    /// `java.util.Random`.
    Legacy(LegacyRandom),
}

/// Either family of factory.
#[derive(Debug, Clone)]
pub enum RandomSplitter {
    /// Makes [`Xoroshiro`] generators.
    Xoroshiro(XoroshiroSplitter),
    /// Makes [`LegacyRandom`] generators.
    Legacy(LegacyRandomSplitter),
}

impl Random for RandomSource {
    fn fork(&mut self) -> Self {
        match self {
            Self::Xoroshiro(random) => Self::Xoroshiro(random.fork()),
            Self::Legacy(random) => Self::Legacy(random.fork()),
        }
    }

    fn next_i32(&mut self) -> i32 {
        match self {
            Self::Xoroshiro(random) => random.next_i32(),
            Self::Legacy(random) => random.next_i32(),
        }
    }

    fn next_i32_bounded(&mut self, bound: i32) -> i32 {
        match self {
            Self::Xoroshiro(random) => random.next_i32_bounded(bound),
            Self::Legacy(random) => random.next_i32_bounded(bound),
        }
    }

    fn next_i64(&mut self) -> i64 {
        match self {
            Self::Xoroshiro(random) => random.next_i64(),
            Self::Legacy(random) => random.next_i64(),
        }
    }

    fn next_f32(&mut self) -> f32 {
        match self {
            Self::Xoroshiro(random) => random.next_f32(),
            Self::Legacy(random) => random.next_f32(),
        }
    }

    fn next_f64(&mut self) -> f64 {
        match self {
            Self::Xoroshiro(random) => random.next_f64(),
            Self::Legacy(random) => random.next_f64(),
        }
    }

    fn next_bool(&mut self) -> bool {
        match self {
            Self::Xoroshiro(random) => random.next_bool(),
            Self::Legacy(random) => random.next_bool(),
        }
    }

    fn next_gaussian(&mut self) -> f64 {
        match self {
            Self::Xoroshiro(random) => random.next_gaussian(),
            Self::Legacy(random) => random.next_gaussian(),
        }
    }

    fn next_positional(&mut self) -> RandomSplitter {
        match self {
            Self::Xoroshiro(random) => random.next_positional(),
            Self::Legacy(random) => random.next_positional(),
        }
    }

    fn consume_count(&mut self, count: i32) {
        match self {
            Self::Xoroshiro(random) => random.consume_count(count),
            Self::Legacy(random) => random.consume_count(count),
        }
    }
}

impl PositionalRandom for RandomSplitter {
    fn at(&self, x: i32, y: i32, z: i32) -> RandomSource {
        match self {
            Self::Xoroshiro(splitter) => splitter.at(x, y, z),
            Self::Legacy(splitter) => splitter.at(x, y, z),
        }
    }

    fn with_hash_of(&self, hash: &NameHash) -> RandomSource {
        match self {
            Self::Xoroshiro(splitter) => splitter.with_hash_of(hash),
            Self::Legacy(splitter) => splitter.with_hash_of(hash),
        }
    }

    fn with_seed(&self, seed: u64) -> RandomSource {
        match self {
            Self::Xoroshiro(splitter) => splitter.with_seed(seed),
            Self::Legacy(splitter) => splitter.with_seed(seed),
        }
    }
}

/// The number the game makes of a block position (`Mth.getSeed`), which the factories
/// mix into their seed. Block models and textures are varied with it as well.
#[must_use]
pub fn get_seed(x: i32, y: i32, z: i32) -> i64 {
    let mixed = i64::from(x.wrapping_mul(3_129_871))
        ^ (i64::from(z).wrapping_mul(116_129_781_i64))
        ^ i64::from(y);
    let mixed = mixed
        .wrapping_mul(mixed)
        .wrapping_mul(42_317_861_i64)
        .wrapping_add(mixed.wrapping_mul(11));
    mixed >> 16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_of_either_family_gives_what_the_generator_inside_it_gives() {
        let mut plain = Xoroshiro::from_seed(7);
        let mut wrapped = RandomSource::Xoroshiro(Xoroshiro::from_seed(7));
        assert_eq!(plain.next_i64(), wrapped.next_i64());
        assert_eq!(plain.next_i32_bounded(100), wrapped.next_i32_bounded(100));
        assert_eq!(plain.next_f64().to_bits(), wrapped.next_f64().to_bits());
        assert_eq!(
            plain.next_gaussian().to_bits(),
            wrapped.next_gaussian().to_bits()
        );
        assert_eq!(plain.fork().next_i64(), wrapped.fork().next_i64());

        let mut plain = LegacyRandom::from_seed(7);
        let mut wrapped = RandomSource::Legacy(LegacyRandom::from_seed(7));
        plain.consume_count(262);
        wrapped.consume_count(262);
        assert_eq!(plain.next_i64(), wrapped.next_i64());
        assert_eq!(plain.next_f32().to_bits(), wrapped.next_f32().to_bits());
        assert_eq!(plain.next_bool(), wrapped.next_bool());
    }

    #[test]
    fn the_seed_of_a_position_wraps_as_java_does() {
        assert_eq!(get_seed(0, 0, 0), 0);
        // Worked by hand with Java's wrapping arithmetic: x * 3129871 is taken in 32
        // bits before it is widened, the rest in 64.
        let x = i64::from(1_000_000_i32.wrapping_mul(3_129_871));
        let mixed = x ^ (-7_i64).wrapping_mul(116_129_781) ^ 64;
        let expected = mixed
            .wrapping_mul(mixed)
            .wrapping_mul(42_317_861)
            .wrapping_add(mixed.wrapping_mul(11))
            >> 16;
        assert_eq!(get_seed(1_000_000, 64, -7), expected);
    }
}
