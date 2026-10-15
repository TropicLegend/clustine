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

pub mod context;
pub mod dispatch;
pub mod feature;
pub mod features;
pub mod placement;
pub mod reader;
mod run;
mod settings;

pub use run::decorate;
pub use settings::{FeatureSettings, STEPS};
