//! The skeleton (`Skeleton`; `n7k-brief.md` 11.2), part K4.
//! A stub until its part fills it; it is in `built` only once it is.

use super::MobType;

/// The skeleton.
#[expect(
    dead_code,
    reason = "its part registers it in `built` when it is filled"
)]
pub(crate) struct Skeleton;

impl MobType for Skeleton {}
