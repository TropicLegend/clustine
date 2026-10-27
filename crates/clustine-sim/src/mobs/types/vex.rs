//! The vex (`Vex`; `docs/groundwork/n9-plan.md`, section 8.4): part S8's,
//! with the evoker that summons it.
//!
//! Until S8 makes it whole, the type takes every default and is not built
//! (`BUILT`), so that an egg of it makes nothing.

use super::MobType;

/// Whether the type is whole, so that `built()` gives it and an egg makes it: S8 sets
/// it when it is.
pub(crate) const BUILT: bool = false;

/// The vex.
pub(crate) struct Vex;

impl MobType for Vex {}
