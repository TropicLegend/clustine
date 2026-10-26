//! The iron golem (`IronGolem`; `n7k-brief.md` 1.2 and 1.3), part K1.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The iron golem.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct IronGolem;

impl MobType for IronGolem {}
