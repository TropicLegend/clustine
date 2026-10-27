//! The crossbow (`CrossbowItem`; `docs/groundwork/n9-plan.md`, section 4.6; part S4's):
//! its charge by the ticks it was held (Quick Charge), the projectiles kept in
//! `charged_projectiles`, and the shot of each (Multishot, Piercing), arrows and rockets.

use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind};

/// `CrossbowItem.getChargeDuration`: 1.25 seconds, a quarter of a second less a level
/// of Quick Charge (its `crossbow_charge_time` effect), in ticks, rounded down.
pub(crate) fn charge_duration(stack: &ItemStack) -> i32 {
    let level = super::enchantments::level(stack, "minecraft:quick_charge");
    let seconds = 1.25_f32 - 0.25 * level as f32;
    (seconds * 20.0).floor() as i32
}

/// `CrossbowItem.isCharged`: its `charged_projectiles` hold something.
pub(crate) fn is_charged(stack: &ItemStack) -> bool {
    matches!(
        stack.components.get(Kind::ChargedProjectiles),
        Some(Component::ChargedProjectiles(projectiles)) if !projectiles.is_empty()
    )
}
