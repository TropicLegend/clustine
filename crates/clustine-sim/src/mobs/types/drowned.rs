//! The drowned (`Drowned`; `n7k-brief.md` 11.1), part K3.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The drowned.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct Drowned;

impl MobType for Drowned {}
