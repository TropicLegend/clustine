//! The wither skeleton (`WitherSkeleton`): family E3 of P12 (`docs/groundwork/p-plan.md`, sections 18.3, 18.5 and
//! 18.6).
//!
//! Until E3 makes it whole, the type takes every default and is not built (`BUILT`),
//! so that no egg, spawner or structure makes one.

use super::MobType;

/// Whether the type is whole, so that `built()` gives it: E3 sets it when it is.
pub(crate) const BUILT: bool = false;

/// The wither skeleton.
pub(crate) struct WitherSkeleton;

impl MobType for WitherSkeleton {}
