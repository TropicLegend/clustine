//! The witch (`Witch`; `n7k-brief.md` 3.6), part K5.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The witch.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct Witch;

impl MobType for Witch {}
