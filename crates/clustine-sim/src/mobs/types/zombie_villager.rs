//! The zombie villager (`ZombieVillager`; `n7k-brief.md` 3.4.3), part K3.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The zombie villager.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct ZombieVillager;

impl MobType for ZombieVillager {}
