//! The piglin brute (`PiglinBrute`): family E5 of P12 (`docs/groundwork/p-plan.md`, sections 18.3, 18.5 and
//! 18.6).
//!
//! Until E5 makes it whole, the type takes every default and is not built (`BUILT`),
//! so that no egg, spawner or structure makes one.

use super::MobType;

/// Whether the type is whole, so that `built()` gives it: E5 sets it when it is.
pub(crate) const BUILT: bool = false;

/// The piglin brute.
pub(crate) struct PiglinBrute;

impl MobType for PiglinBrute {}
