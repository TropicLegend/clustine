//! The enchantments N9's rules read (`docs/groundwork/n9-plan.md`, section 4.10; part
//! S4's): each a rule written from its data file's values (Power, Punch, Flame, Infinity,
//! Multishot, Piercing, Quick Charge, Loyalty, Impaling, and Unbreaking and Mending for
//! wear), read from a stack's `enchantments` component. The reads below are what the
//! arrows (S2) and the weapons (S4) call.

use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind};

/// The level of the enchantment `name` (`minecraft:power`, say) on `stack`, 0 where it
/// has none (`EnchantmentHelper.getItemEnchantmentLevel`). An item's defaults hold no
/// enchantments, so only the patch is read.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the arrows (S2) and the weapons (S4) read it")
)]
pub(crate) fn level(stack: &ItemStack, name: &str) -> i32 {
    match stack.components.get(Kind::Enchantments) {
        Some(Component::Enchantments(enchantments)) => enchantments
            .iter()
            .find(|(enchantment, _)| enchantment == name)
            .map_or(0, |(_, level)| *level),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stack_has_the_level_of_an_enchantment_it_holds_and_none_of_another() {
        let mut bow = ItemStack::new(clustine_data::items::BOW, 1);
        assert_eq!(level(&bow, "minecraft:power"), 0);
        bow.components.set(Component::Enchantments(vec![
            ("minecraft:power".to_owned(), 3),
            ("minecraft:flame".to_owned(), 1),
        ]));
        assert_eq!(level(&bow, "minecraft:power"), 3);
        assert_eq!(level(&bow, "minecraft:flame"), 1);
        assert_eq!(level(&bow, "minecraft:punch"), 0);
    }
}
