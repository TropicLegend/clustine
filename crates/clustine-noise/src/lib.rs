//! Minecraft's random number generators and noises, bit for bit as the game computes
//! them: world generation is equal to the game's only if every one of these is.
//!
//! Nothing here uses the platform's mathematics library: `sin`, `exp` and the like
//! differ in the last bit between platforms, and every Clustine process has to agree.
//! They go through `libm`, except the logarithm, for which `libm`'s is not the game's
//! often enough to matter ([`math::ln`]). Nothing here uses a hash map, a clock or
//! I/O either.
//!
//! What is here, and what the game uses it for:
//!
//! - [`random`]: the generators. [`Xoroshiro`] for today's dimensions,
//!   [`LegacyRandom`] (`java.util.Random`) for what predates them, [`WorldgenRandom`]
//!   for placing features, and the factories that give a generator for a name or a
//!   block position ([`RandomSplitter`]).
//! - [`ImprovedNoise`]: one octave of Perlin noise. [`NormalNoise`]: the named noises
//!   of a noise router (temperature, erosion, caves, aquifers, ore veins).
//!   [`BlendedNoise`]: the base of the terrain. [`PerlinNoise`]: octaves summed in
//!   double precision, for the older noises.
//! - [`SimplexNoise`] and [`PerlinSimplexNoise`]: the End's islands, and the noises
//!   that vary a biome's temperature and tint.
//! - [`spline`]: the cubic splines that shape the land from the climate noises.
//! - [`math`], [`trig`], [`angle`]: the game's `Mth`: floors, interpolations, the
//!   table of sines that carvers and features use.
//!
//! In single precision and without contraction: since 26.3 the game computes Perlin
//! noise in floats, and each function here keeps the game's width and order for every
//! operation. Rust never fuses a multiplication and an addition on its own, so the
//! results are the same on every target.
//!
//! See `docs/groundwork/terrain-plan.md`, step T1, and `NOTICE` for where the code is
//! adapted from.

pub mod angle;
mod blended_noise;
mod improved_noise;
mod ln;
pub mod math;
mod normal_noise;
mod perlin_noise;
mod perlin_simplex_noise;
pub mod random;
mod simplex_noise;
pub mod spline;
pub mod trig;

pub use blended_noise::BlendedNoise;
pub use improved_noise::ImprovedNoise;
pub use normal_noise::NormalNoise;
pub use perlin_noise::PerlinNoise;
pub use perlin_simplex_noise::PerlinSimplexNoise;
pub use random::{
    LegacyRandom, LegacyRandomSplitter, NameHash, PositionalRandom, Random, RandomSource,
    RandomSplitter, WorldgenRandom, Xoroshiro, XoroshiroSplitter,
};
pub use simplex_noise::SimplexNoise;
