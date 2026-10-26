//! The parched (`Parched`; `n7k-brief.md` 11.2), part K4.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The parched.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct Parched;

impl MobType for Parched {}
