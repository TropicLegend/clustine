//! The slime (`Slime` and `AbstractCubeMob`; `docs/groundwork/n9-plan.md`, section
//! 8.4): part S9's, on the cube's layer in `cube_mob`.
//!
//! Until S9 makes it whole, the type takes every default and is not built
//! (`BUILT`), so that an egg of it makes nothing.

use super::MobType;

/// Whether the type is whole, so that `built()` gives it and an egg makes it: S9 sets
/// it when it is.
pub(crate) const BUILT: bool = false;

/// The slime.
pub(crate) struct Slime;

impl MobType for Slime {}
