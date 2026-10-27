//! The rules of items in a hand that N9 adds (`docs/groundwork/n9-plan.md`, sections 4.6,
//! 4.10 and 5.5): the bow, the crossbow and the trident, the throwables, a rocket used on
//! a block, the enchantments the rules read, and eating and drinking. Each file is its
//! part's: S4's but `consumables.rs`, which is S1's. The use that starts and ends a
//! draw, a charge or a meal is the region's use state (N9c).

pub(crate) mod bow;
pub(crate) mod consumables;
pub(crate) mod crossbow;
pub(crate) mod enchantments;
#[cfg(test)]
mod items_n9_tests;
pub(crate) mod rockets;
pub(crate) mod throwables;
pub(crate) mod trident;
