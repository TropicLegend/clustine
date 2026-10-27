//! The trident in a hand (`TridentItem`; `docs/groundwork/n9-plan.md`, section 4.6; part
//! S4's): thrown at its release after ten ticks held, with its enchantments (Loyalty);
//! Riptide is named.
//!
//! Named difference: Riptide's spin is not built (decision 1 of the plan), so a trident
//! with Riptide is not thrown at all, as the game throws none (n9 TridentItem 61–277:
//! with a spin strength above 0 the release either spins the player in water or rain or
//! does nothing).

use clustine_world::ItemStack;

use super::enchantments;

/// `THROW_THRESHOLD_TIME`: the ticks it must be held for before a release throws it.
pub(crate) const THROW_THRESHOLD: i32 = 10;

/// `PROJECTILE_SHOOT_POWER`, and the throw's roll and uncertainty
/// (`spawnProjectileFromRotation(ThrownTrident::new, …, 0, 2.5f, 1.0f)`).
pub(crate) const POWER: f32 = 2.5;
pub(crate) const UNCERTAINTY: f32 = 1.0;

/// Whether a release after `ticks` held throws the trident `stack`: held long enough,
/// without Riptide, and its next wear not breaking it (`releaseUsing`'s refusals).
pub(crate) fn throws(stack: &ItemStack, ticks: i32) -> bool {
    ticks >= THROW_THRESHOLD
        && enchantments::riptide_strength(stack) <= 0.0
        && !enchantments::next_damage_will_break(stack)
}

/// `TridentItem.use`: whether a use may begin (not where its next wear breaks it, nor
/// with Riptide out of water and rain, which Clustine does not have).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the start of a use (using.rs) is to ask it")
)]
pub(crate) fn can_raise(stack: &ItemStack, in_water: bool) -> bool {
    !enchantments::next_damage_will_break(stack)
        && (enchantments::riptide_strength(stack) <= 0.0 || in_water)
}
