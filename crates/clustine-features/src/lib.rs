//! Minecraft's features: trees, plants, ores, lakes and the rest of the game's
//! features step, placed into the view of one feature run as the game places them
//! (`docs/groundwork/f-plan.md`; the order of runs and the view are
//! `docs/adr/0021-generation-is-a-function.md`'s).
//!
//! [`FeatureSettings`] reads a dimension's features from `clustine-worldgen-data`
//! once; [`decorate`] places a chunk's. The kinds of features are built step by step
//! (`dispatch`); one not built yet places nothing, as the game's `no_op`.
//!
//! Like the simulation, this is a function of its inputs: no clock, no hash map, no
//! I/O.

pub mod blocks;
pub mod context;
pub mod dispatch;
pub mod feature;
pub mod features;
pub mod java_set;
pub mod placement;
pub mod predicates;
pub mod providers;
pub mod reader;
mod run;
mod settings;
pub mod survive;
#[cfg(test)]
mod testing;
pub mod values;

pub use run::decorate;

/// How many of the plan's steps that build kinds of features are in (section 3: F1b,
/// F2, F3, F4, F5 and the overworld's last in F6), raised by each. It is part of the
/// identity of what the features' stages make, so that a world made with fewer is
/// refused rather than continued with more.
pub const GENERATION: u16 = 5;
pub use settings::{FeatureSettings, STEPS};
