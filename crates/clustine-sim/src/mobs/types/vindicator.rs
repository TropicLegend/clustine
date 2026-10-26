//! The vindicator (`Vindicator`; `n7k-brief.md` 3.7), part K5.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The vindicator.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct Vindicator;

impl MobType for Vindicator {}
