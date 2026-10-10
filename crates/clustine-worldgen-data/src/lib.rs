//! Minecraft's world-generation data, turned into Rust by `cargo datagen`.
//!
//! What is here so far is the part that shapes the terrain: for the overworld, the
//! Nether and the End, the noise router of the dimension's noise settings with every
//! density function it reaches, as emitted Rust ([`OverworldRouter`], [`NetherRouter`],
//! [`EndRouter`]); the splines of those functions, the parameters of every noise
//! ([`noises`]) and the rest of the three noise settings ([`noise_settings`]) as
//! statics. Material rules, biomes, features and structures come with later steps of
//! `docs/groundwork/terrain-plan.md`.
//!
//! A router is made from a world's seed and gives each of its functions at a block
//! position, through the noises of `clustine-noise`:
//!
//! ```
//! use clustine_worldgen_data::{NoiseRouter, OverworldRouter, RouterEntry};
//!
//! let router = OverworldRouter::new(13579);
//! let mut memory = Default::default();
//! let density = router.sample(&mut memory, RouterEntry::FinalDensity, 10, 70, -33);
//! assert!(density < 0.0, "there is air at that block");
//! ```
//!
//! Everything under `generated` is written by `cargo datagen` and must not be edited
//! by hand; `docs/adr/0019-data-made-from-mojangs-jar.md` says what is emitted in
//! which form and how `cargo datagen --check` proves it. The data in the generated
//! files and under `reference/` is Mojang's and not under the licence of the code
//! around it; see `NOTICE.md` at the root of the repository. Parts of the code are
//! adapted from SteelMC; see `NOTICE` in this crate.
//!
//! Like the simulation, this is a function of its inputs: no clock, no hash map, no
//! I/O, and no mathematics of the platform.

pub mod density;

// The generated code is laid out by the emitter and never formatted, so that its bytes
// do not depend on a toolchain (ADR-0019, section 1).
#[rustfmt::skip]
// It is emitted: a lint would be answered in the emitter or not at all.
#[allow(clippy::all)]
mod generated;

pub use density::{AquiferEntry, NoiseParameters, NoiseRouter, NoiseSettings, RouterEntry};
pub use generated::end::router::EndRouter;
pub use generated::nether::router::NetherRouter;
pub use generated::overworld::router::OverworldRouter;
pub use generated::{noise_settings, noises};
