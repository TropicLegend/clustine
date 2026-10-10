//! Minecraft's random number generators and noises, bit for bit as the game computes
//! them: world generation is equal to the game's only if every one of these is.
//!
//! Nothing here uses the platform's mathematics library: `sin`, `exp` and the like
//! differ in the last bit between platforms, and every Clustine process has to agree.
//! They go through `libm`. Nothing here uses a hash map, a clock or I/O either.
//!
//! See `docs/groundwork/terrain-plan.md`, step T1, and `NOTICE` for where the code is
//! adapted from.
