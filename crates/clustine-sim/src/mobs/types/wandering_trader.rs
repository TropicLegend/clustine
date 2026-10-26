//! The wandering trader (`WanderingTrader`; `n7k-brief.md` 2.7), part K2.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The wandering trader.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct WanderingTrader;

impl MobType for WanderingTrader {}
