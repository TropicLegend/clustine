//! Minecraft's terrain as a function of a world's seed and a position.
//!
//! So far this is the part every other part asks, and the first three steps of a
//! chunk's blocks, each for the overworld, the Nether and the End:
//!
//! - [`biome`]: which biome is where, in cells of four blocks, and which cell a block
//!   takes its biome from ([`biome::zoom`]).
//! - [`fill`]: the game's "fill from noise" step, which gives a chunk its ground, its
//!   air and its fluids, and the height of the ground at a column without filling a
//!   chunk. It is made of [`noise_chunk`], the final density of a box of blocks as
//!   the game fills it, and [`aquifer`], which says what lies where no ground is.
//!   Every block of 5,600 chunks equals what the game's own classes give when asked
//!   without a server; the head of [`fill`] says more.
//! - [`surface`]: the game's "build surface" step, which makes of the ground what the
//!   dimension's material rule says: grass, sand, snow, the bands of the badlands,
//!   bedrock, deepslate, and the ore veins. It was compared the same way; the head of
//!   [`surface`] says more.
//! - [`carver`]: the game's "generate carvers" step, which cuts caves and ravines
//!   into the ground: which blocks the carvers of the chunks up to eight away reach
//!   ([`carver::Mask`]), and what becomes of them (air, or the water or lava the
//!   chunk's aquifer has there; grass again on dirt that was laid bare). Compared the
//!   same way: every block of 4,780 chunks. The head of [`carver`] says more.
//!
//! - [`spawn`]: where the game begins to look for a new world's spawn, from the
//!   climate.
//! - [`beard`]: what structures do to the ground, as a number the fill adds to the
//!   density near their pieces (the game's `Beardifier`). Which pieces are near a
//!   chunk is `clustine-structures`' to say.
//!
//! [`fill::Terrain::carved`] is the three steps together: the chunk as the game's
//! `buildTerrain` leaves it where no structure bends the ground, and
//! [`fill::Terrain::carved_bent`] where structures do: every block of 9,875 chunks
//! under villages, camps, outposts, ancient cities, trail ruins, trial chambers,
//! strongholds and nether fossils equals the game's (`clustine-structures`,
//! `tests/game_bent.rs`). The features come with the later steps of
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
//! ```
//! use clustine_terrain::biome::Dimension;
//! use clustine_terrain::fill::Terrain;
//!
//! let nether = Terrain::new(13579, Dimension::Nether);
//! let filled = nether.fill(0, 0);
//! // Netherrack at the bottom, and a sea of lava below the height 32.
//! assert_eq!(filled.get(0, 0, 0), clustine_data::BlockState(8667));
//! assert_eq!(filled.get(0, 25, 0), clustine_data::blocks::LAVA);
//! ```
//!
//! Like the simulation, this is a function of its inputs (ADR-0021): no clock, no
//! thread, no hash map, no I/O, and nothing remembered from one call to the next that
//! could change an answer. Parts of the code are adapted from SteelMC; see `NOTICE` in
//! this crate. The data it computes with is Mojang's, from `clustine-data` and
//! `clustine-worldgen-data`.

pub mod aquifer;
pub mod beard;
pub mod biome;
pub mod carver;
pub mod fill;
pub mod noise_chunk;
pub mod spawn;
pub mod surface;
