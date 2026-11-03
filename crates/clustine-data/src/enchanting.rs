//! What enchanting reads of the enchantments and the items, compiled by `cargo datagen`
//! from the game's `data/minecraft/enchantment/*.json` and the items' `minecraft:enchantable`
//! (`docs/groundwork/n10-plan.md`, decision 8): each enchantment's weight, levels, costs,
//! exclusive set and the items it supports and is primary for, and each item's
//! enchantability; and the slots each works from with its `minecraft:attributes` effects.
//! The rules that pick enchantments are `clustine-sim`'s (`items/enchanting.rs`); the
//! other effects of an enchantment are the rules' own.
//!
//! The tags the sets name are the generated [`crate::TAGS`], each in its file's order with
//! a nested tag expanded where it stands, as the game's tag loader builds a `HolderSet`;
//! enchantments in them are numbered by the synchronised registry, which is
//! [`enchantments`]'s order.

use crate::generated::enchantments::{ENCHANTABLE, ENCHANTMENTS};
use crate::trading::{ItemSet, NamedSet};
use crate::{TAGS, Tag};

/// `Enchantment$Cost`: `base + per_level_above_first · (level − 1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnchantmentCost {
    pub base: i32,
    pub per_level_above_first: i32,
}

impl EnchantmentCost {
    /// `calculate(level)`, in Java's wrapping `int`s.
    pub const fn calculate(self, level: i32) -> i32 {
        self.base.wrapping_add(
            self.per_level_above_first
                .wrapping_mul(level.wrapping_sub(1)),
        )
    }
}

/// A `LevelBasedValue` of the shapes the attribute effects use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LevelBased {
    Constant(f32),
    /// `base + per_level_above_first · (level − 1)`, in floats.
    Linear {
        base: f32,
        per_level_above_first: f32,
    },
    /// `level² + added`.
    LevelsSquared {
        added: f32,
    },
    /// The numerator over the denominator, 0 where the denominator is.
    Fraction {
        numerator: &'static LevelBased,
        denominator: &'static LevelBased,
    },
}

impl LevelBased {
    /// `calculate(level)`.
    pub fn calculate(self, level: i32) -> f32 {
        match self {
            Self::Constant(value) => value,
            Self::Linear {
                base,
                per_level_above_first,
            } => base + per_level_above_first * level.wrapping_sub(1) as f32,
            Self::LevelsSquared { added } => level.wrapping_mul(level) as f32 + added,
            Self::Fraction {
                numerator,
                denominator,
            } => {
                let denominator = denominator.calculate(level);
                if denominator == 0.0 {
                    0.0
                } else {
                    numerator.calculate(level) / denominator
                }
            }
        }
    }
}

/// One of an enchantment's `minecraft:attributes` effects (`EnchantmentAttributeEffect`):
/// a modifier its holder's attribute takes while the enchanted item is in one of the
/// enchantment's slots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnchantmentAttribute {
    /// The attribute, by its id in [`crate::ATTRIBUTES`].
    pub attribute: u16,
    /// The modifier's id before the slot's name is added (`idForSlot`).
    pub id: &'static str,
    pub amount: LevelBased,
    /// `AttributeModifier.Operation`'s ordinal.
    pub operation: u8,
}

/// What enchanting reads of one enchantment (`Enchantment$EnchantmentDefinition` and the
/// enchantment's exclusive set).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Enchantment {
    /// Its name in the registry, such as `minecraft:sharpness`.
    pub name: &'static str,
    pub weight: i32,
    pub max_level: i32,
    pub min_cost: EnchantmentCost,
    pub max_cost: EnchantmentCost,
    /// The enchantments it does not go with; none where the file names none.
    pub exclusive_set: Option<NamedSet>,
    pub supported_items: ItemSet,
    /// Where there is a set, the items an enchanting table offers it for.
    pub primary_items: Option<ItemSet>,
    /// The slot groups it works from, by `EquipmentSlotGroup`'s ordinal (as
    /// [`crate::ItemModifier::slot`]).
    pub slots: &'static [u8],
    pub attributes: &'static [EnchantmentAttribute],
}

impl Enchantment {
    /// `matchingSlot(slot)`: whether it works from the `EquipmentSlot` of ordinal
    /// `slot`.
    pub fn matching_slot(&self, slot: u8) -> bool {
        self.slots
            .iter()
            .any(|&group| crate::slot_group_holds(group, slot))
    }

    /// `getMinLevel()`: always 1.
    pub const fn min_level(&self) -> i32 {
        1
    }

    /// `getMinCost(level)`.
    pub const fn min_cost(&self, level: i32) -> i32 {
        self.min_cost.calculate(level)
    }

    /// `getMaxCost(level)`.
    pub const fn max_cost(&self, level: i32) -> i32 {
        self.max_cost.calculate(level)
    }

    /// `isSupportedItem(stack)` and `canEnchant(stack)`: the item is of the supported
    /// set.
    pub fn is_supported_item(&self, item: i32) -> bool {
        item_in(&self.supported_items, item)
    }

    /// `isPrimaryItem(stack)`: supported, and of the primary set where there is one.
    pub fn is_primary_item(&self, item: i32) -> bool {
        self.is_supported_item(item)
            && self
                .primary_items
                .as_ref()
                .is_none_or(|primary| item_in(primary, item))
    }

    /// Whether `other` is in this one's exclusive set.
    pub fn excludes(&self, other: &str) -> bool {
        self.exclusive_set
            .as_ref()
            .is_some_and(|set| enchantment_in(set, other))
    }
}

/// `Enchantment.areCompatible(a, b)`: not the same enchantment, and neither in the
/// other's exclusive set.
pub fn are_compatible(a: &Enchantment, b: &Enchantment) -> bool {
    a.name != b.name && !a.excludes(b.name) && !b.excludes(a.name)
}

/// Every enchantment, in the registry's order (by name): an enchantment's place is its
/// number in the synchronised registry.
pub fn enchantments() -> &'static [Enchantment] {
    &ENCHANTMENTS
}

/// The enchantment of this name.
pub fn enchantment(name: &str) -> Option<&'static Enchantment> {
    let at = ENCHANTMENTS
        .binary_search_by(|enchantment| enchantment.name.cmp(name))
        .ok()?;
    Some(&ENCHANTMENTS[at])
}

/// The item's `minecraft:enchantable` by default (`Enchantable.value`), where it has one.
pub fn enchantability(item: i32) -> Option<i32> {
    let item = u16::try_from(item).ok()?;
    let at = ENCHANTABLE
        .binary_search_by_key(&item, |(id, _)| *id)
        .ok()?;
    Some(ENCHANTABLE[at].1)
}

/// The tag `name` (with or without its `#`) of the registry `registry`.
pub(crate) fn tag(registry: &str, name: &str) -> Option<&'static Tag> {
    let name = name.strip_prefix('#').unwrap_or(name);
    TAGS.iter()
        .find(|tags| tags.registry == registry)?
        .tags
        .iter()
        .find(|tag| tag.name == name)
}

/// Whether the item of id `item` is in `set` (`ItemStack.is(HolderSet)`). A tag the data
/// has not got holds nothing.
pub fn item_in(set: &ItemSet, item: i32) -> bool {
    match set {
        ItemSet::Tag(name) => {
            tag("minecraft:item", name).is_some_and(|tag| tag.entries.contains(&item))
        }
        ItemSet::Listed(ids) => ids.iter().any(|&id| i32::from(id) == item),
    }
}

/// The enchantments of `set` in its order (`HolderSet.stream()`): a tag's in the tag's
/// order, listed ones as listed.
pub fn enchantments_in(set: &NamedSet) -> Vec<&'static Enchantment> {
    match set {
        NamedSet::Tag(name) => tag("minecraft:enchantment", name)
            .map(|tag| {
                tag.entries
                    .iter()
                    .filter_map(|&id| ENCHANTMENTS.get(usize::try_from(id).ok()?))
                    .collect()
            })
            .unwrap_or_default(),
        NamedSet::Listed(names) => names.iter().filter_map(|name| enchantment(name)).collect(),
    }
}

/// Whether the enchantment `name` is in `set` (`HolderSet.contains`).
pub fn enchantment_in(set: &NamedSet, name: &str) -> bool {
    match set {
        NamedSet::Tag(tag_name) => {
            let Ok(id) = ENCHANTMENTS.binary_search_by(|enchantment| enchantment.name.cmp(name))
            else {
                return false;
            };
            tag("minecraft:enchantment", tag_name).is_some_and(|tag| {
                tag.entries
                    .iter()
                    .any(|&entry| usize::try_from(entry) == Ok(id))
            })
        }
        NamedSet::Listed(names) => names.contains(&name),
    }
}

/// The names of the entries of the registry `registry`'s tag `name`, in the tag's order,
/// for a registry whose names the caller numbers by `names` (the potions', say).
pub fn tag_names(registry: &str, name: &str, names: &[&'static str]) -> Vec<&'static str> {
    tag(registry, name)
        .map(|tag| {
            tag.entries
                .iter()
                .filter_map(|&id| names.get(usize::try_from(id).ok()?).copied())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{items, synced_registry};

    /// The table is the synchronised registry's entries in its order, so that the
    /// numbers in the enchantment tags are places in it.
    #[test]
    fn the_enchantments_are_the_registry_s_in_its_order() {
        let registry = synced_registry("minecraft:enchantment").expect("enchantments are synced");
        let names: Vec<&str> = enchantments().iter().map(|e| e.name).collect();
        assert_eq!(names, registry.entries);
        assert_eq!(enchantments().len(), 43);
    }

    /// The modifiers of `data/minecraft/enchantment/*.json`'s `minecraft:attributes`,
    /// at each level, as `EnchantmentAttributeEffect.getModifier` makes them.
    #[test]
    fn the_attribute_effects_give_the_game_s_amounts_from_their_slots() {
        let effect = |name: &str| {
            let enchantment = enchantment(name).unwrap();
            assert_eq!(enchantment.attributes.len(), 1, "{name}");
            (enchantment, enchantment.attributes[0])
        };
        let (efficiency, mining) = effect("minecraft:efficiency");
        assert_eq!(
            crate::ATTRIBUTES[usize::from(mining.attribute)].name,
            "minecraft:mining_efficiency"
        );
        assert_eq!(mining.id, "minecraft:enchantment.efficiency");
        assert_eq!(mining.operation, 0);
        let levels: Vec<f32> = (1..=5)
            .map(|level| mining.amount.calculate(level))
            .collect();
        assert_eq!(levels, [2.0, 5.0, 10.0, 17.0, 26.0]);
        assert!(efficiency.matching_slot(0));
        assert!(!efficiency.matching_slot(1));

        let (aqua_affinity, submerged) = effect("minecraft:aqua_affinity");
        assert_eq!(
            crate::ATTRIBUTES[usize::from(submerged.attribute)].name,
            "minecraft:submerged_mining_speed"
        );
        assert_eq!(submerged.amount.calculate(1), 4.0);
        assert_eq!(submerged.operation, 2);
        assert!(aqua_affinity.matching_slot(5));
        assert!(!aqua_affinity.matching_slot(0));

        let (_, sweeping) = effect("minecraft:sweeping_edge");
        assert_eq!(sweeping.amount.calculate(1), 0.5);
        assert_eq!(sweeping.amount.calculate(3), 0.75);

        // `armor`: the four humanoid slots and the body, not the hands or a saddle.
        let (blast_protection, _) = effect("minecraft:blast_protection");
        let worn: Vec<u8> = (0..8)
            .filter(|&slot| blast_protection.matching_slot(slot))
            .collect();
        assert_eq!(worn, [2, 3, 4, 5, 6]);
        let (_, depth_strider) = effect("minecraft:depth_strider");
        assert_eq!(depth_strider.amount.calculate(3), 0.333_333_34 * 3.0);
        assert!(
            enchantment("minecraft:sharpness")
                .unwrap()
                .attributes
                .is_empty()
        );
    }

    /// `data/minecraft/enchantment/sharpness.json` and `mending.json`, read back.
    #[test]
    fn sharpness_and_mending_are_as_their_files_have_them() {
        let sharpness = enchantment("minecraft:sharpness").unwrap();
        assert_eq!(sharpness.weight, 10);
        assert_eq!(sharpness.max_level, 5);
        assert_eq!((sharpness.min_cost(1), sharpness.max_cost(1)), (1, 21));
        assert_eq!((sharpness.min_cost(5), sharpness.max_cost(5)), (45, 65));
        assert!(sharpness.is_primary_item(items::DIAMOND_SWORD));
        // An axe takes sharpness, but a table offers it only for melee weapons' primary set.
        assert!(sharpness.is_supported_item(items::IRON_AXE));
        assert!(!sharpness.is_supported_item(items::BOOK));
        let smite = enchantment("minecraft:smite").unwrap();
        assert!(!are_compatible(sharpness, smite));
        assert!(!are_compatible(sharpness, sharpness));
        let mending = enchantment("minecraft:mending").unwrap();
        assert_eq!(mending.weight, 2);
        assert_eq!((mending.min_cost(1), mending.max_cost(1)), (25, 75));
        assert!(are_compatible(sharpness, mending));
        assert!(!are_compatible(
            mending,
            enchantment("minecraft:infinity").unwrap()
        ));
    }

    /// The tags trades name: `#tradeable` holds every non-treasure enchantment and the
    /// four it lists (both curses, frost walker and mending), and `#double_trade_price`
    /// the treasure ones.
    #[test]
    fn the_tradeable_tag_holds_the_non_treasure_and_four_more() {
        let tradeable = enchantments_in(&NamedSet::Tag("#minecraft:tradeable"));
        let names: Vec<&str> = tradeable.iter().map(|e| e.name).collect();
        assert!(names.contains(&"minecraft:sharpness"));
        assert!(names.contains(&"minecraft:mending"));
        assert!(names.contains(&"minecraft:binding_curse"));
        assert!(!names.contains(&"minecraft:soul_speed"));
        assert!(!names.contains(&"minecraft:swift_sneak"));
        assert!(!names.contains(&"minecraft:wind_burst"));
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "each once");
        let double = NamedSet::Tag("#minecraft:double_trade_price");
        assert!(enchantment_in(&double, "minecraft:mending"));
        assert!(enchantment_in(&double, "minecraft:frost_walker"));
        assert!(!enchantment_in(&double, "minecraft:sharpness"));
        assert!(!enchantment_in(&double, "minecraft:nothing"));
    }

    #[test]
    fn a_book_and_tools_have_their_enchantability_and_a_stick_none() {
        assert_eq!(enchantability(items::BOOK), Some(1));
        assert_eq!(enchantability(items::DIAMOND_SWORD), Some(10));
        assert_eq!(enchantability(items::GOLDEN_SWORD), Some(22));
        assert_eq!(enchantability(items::STICK), None);
        assert_eq!(enchantability(items::ENCHANTED_BOOK), None);
        assert_eq!(enchantability(-1), None);
    }
}
