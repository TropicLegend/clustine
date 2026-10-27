//! Trading (`docs/groundwork/n10-plan.md`, section 8): drawing a merchant's offers from
//! the compiled trades (`offers.rs`, the game's `VillagerTrade.getOffer` and
//! `AbstractVillager.addOffersFromTradeSet`) and the item modifiers and number providers
//! the trades use (`evaluate.rs`). The offer's own arithmetic is the world's
//! (`clustine_world::trading`); the merchant's rules and menu are T2's.
//!
//! The random of a draw is the caller's: the game draws each set from a world-wide
//! sequence of its own, which a cluster cannot share, so the villager draws from its
//! stream aside (decision 5, named).

#[allow(
    dead_code,
    reason = "the merchant's rules (T2) and the villager (N10c) call these"
)]
pub(crate) mod evaluate;
#[allow(
    dead_code,
    reason = "the merchant's rules (T2) and the villager (N10c) call these"
)]
pub(crate) mod offers;
#[cfg(test)]
mod trading_n10_tests;
