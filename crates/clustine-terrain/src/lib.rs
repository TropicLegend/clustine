//! Minecraft's terrain as a function of a world's seed and a position.
//!
//! So far this is the part every other part asks: [`biome`], which biome is where in
//! the overworld, the Nether and the End. The shape of the ground, its surface and
//! its caves come with the later steps of phase T of
//! `docs/groundwork/terrain-plan.md`.
//!
//! The overworld's biomes equal those of chunks the official server made, but for
//! exact ties, which the game does not decide by the position. **The Nether's and
//! the End's were not compared with chunks of the official server**, for want of
//! such chunks: they equal what the game's own classes give when asked without a
//! server. The head of [`biome`] has the numbers.
//!
//! ```
//! use clustine_terrain::biome::{BiomeSource, Dimension};
//!
//! let overworld = BiomeSource::new(13579, Dimension::Overworld);
//! // The biomes of the chunk at the origin: 24 sections of 64 cells.
//! let sections = overworld.chunk(0, 0);
//! assert_eq!(sections.len(), 24);
//! // A cell alone, by its position in cells of four blocks.
//! assert_eq!(overworld.biome(0, -16, 0), sections[0][0]);
//! ```
//!
//! Like the simulation, this is a function of its inputs (ADR-0021): no clock, no
//! thread, no hash map, no I/O, and nothing remembered from one call to the next that
//! could change an answer. Parts of the code are adapted from SteelMC; see `NOTICE` in
//! this crate. The data it computes with is Mojang's, from `clustine-data` and
//! `clustine-worldgen-data`.

pub mod biome;
