//! The ender dragon (`EnderDragon`): family E9 of the End's milestone
//! (`docs/groundwork/p-plan.md`, sections 19.2, 19.5 and 19.6).
//!
//! Until E9 makes it whole, the type takes every default and is not built (`BUILT`),
//! so that no egg, spawner or structure makes one.

use super::MobType;

/// Whether the type is whole, so that `built()` gives it: E9 sets it when it is.
pub(crate) const BUILT: bool = false;

/// The ender dragon.
pub(crate) struct EnderDragon;

impl MobType for EnderDragon {}
