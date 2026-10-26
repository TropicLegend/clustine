//! The villager as a body (`Villager`; `n7k-brief.md` section 2), part K2.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The villager.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct Villager;

impl MobType for Villager {}
