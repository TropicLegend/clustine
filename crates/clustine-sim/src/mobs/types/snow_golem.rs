//! The snow golem (`SnowGolem`; `n7k-brief.md` 1.4), part K1.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The snow golem.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct SnowGolem;

impl MobType for SnowGolem {}
