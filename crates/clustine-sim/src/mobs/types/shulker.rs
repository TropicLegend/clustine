//! The shulker (`Shulker`): family E8 of the End's milestone
//! (`docs/groundwork/p-plan.md`, sections 19.2, 19.5 and 19.6).
//!
//! Until E8 makes it whole, the type takes every default and is not built (`BUILT`),
//! so that no egg, spawner or structure makes one.

use super::MobType;

/// Whether the type is whole, so that `built()` gives it: E8 sets it when it is.
pub(crate) const BUILT: bool = false;

/// The shulker.
pub(crate) struct Shulker;

impl MobType for Shulker {}
