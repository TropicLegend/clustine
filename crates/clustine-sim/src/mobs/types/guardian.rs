//! The guardian (`Guardian`; `n7k-brief.md` 3.8), part K5.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The guardian.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct Guardian;

impl MobType for Guardian {}
