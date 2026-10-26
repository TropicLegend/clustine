//! The creeper (`Creeper`; `n7k-brief.md` 11.3), part K4.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The creeper.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct Creeper;

impl MobType for Creeper {}
