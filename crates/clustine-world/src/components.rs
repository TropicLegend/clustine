//! The components an item stack carries beyond its item and count
//! (`docs/groundwork/n9-plan.md`, section 3.1): the game's `DataComponentPatch`, for the
//! set N9 reads. A stack keeps only what differs from its item's defaults (the generated
//! table `clustine_data::ItemComponents`): the components it adds or changes, and the
//! kinds it removes. Nearly every stack has none, so the patch is boxed behind an option
//! and a plain stack costs one pointer.
//!
//! Registry values (a potion, an effect, an enchantment, a banner pattern, a variant)
//! are kept by name, here and in the store, and become the registry's number only on the
//! wire, so that a game update that renumbers a registry needs no format raise. Lists
//! keep the order the game gives them, and the patch keeps its components by kind, so
//! two equal patches are equal field for field and `clustine-sim` stays deterministic.

use serde::{Deserialize, Serialize};

use crate::{Float, ItemStack};

mod structure;
pub use structure::{
    StructureError, Value, component_from, json, stack_from, stack_structure, structure,
};

/// The kinds of component Clustine knows, in the order a patch keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Kind {
    PotionContents,
    PotionDurationScale,
    ChargedProjectiles,
    Enchantments,
    UseCooldown,
    Fireworks,
    Damage,
    IntangibleProjectile,
    BannerPatterns,
    ItemName,
    Rarity,
    TooltipDisplay,
    SuspiciousStewEffects,
    OminousBottleAmplifier,
    ChickenVariant,
    StoredEnchantments,
    DyedColor,
}

impl Kind {
    /// Every kind, in the patch's order.
    pub const ALL: [Kind; 17] = [
        Kind::PotionContents,
        Kind::PotionDurationScale,
        Kind::ChargedProjectiles,
        Kind::Enchantments,
        Kind::UseCooldown,
        Kind::Fireworks,
        Kind::Damage,
        Kind::IntangibleProjectile,
        Kind::BannerPatterns,
        Kind::ItemName,
        Kind::Rarity,
        Kind::TooltipDisplay,
        Kind::SuspiciousStewEffects,
        Kind::OminousBottleAmplifier,
        Kind::ChickenVariant,
        Kind::StoredEnchantments,
        Kind::DyedColor,
    ];

    /// The game's name of the kind in the `data_component_type` registry, which is also
    /// how the store writes it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::PotionContents => "minecraft:potion_contents",
            Kind::PotionDurationScale => "minecraft:potion_duration_scale",
            Kind::ChargedProjectiles => "minecraft:charged_projectiles",
            Kind::Enchantments => "minecraft:enchantments",
            Kind::UseCooldown => "minecraft:use_cooldown",
            Kind::Fireworks => "minecraft:fireworks",
            Kind::Damage => "minecraft:damage",
            Kind::IntangibleProjectile => "minecraft:intangible_projectile",
            Kind::BannerPatterns => "minecraft:banner_patterns",
            Kind::ItemName => "minecraft:item_name",
            Kind::Rarity => "minecraft:rarity",
            Kind::TooltipDisplay => "minecraft:tooltip_display",
            Kind::SuspiciousStewEffects => "minecraft:suspicious_stew_effects",
            Kind::OminousBottleAmplifier => "minecraft:ominous_bottle_amplifier",
            Kind::ChickenVariant => "minecraft:chicken/variant",
            Kind::StoredEnchantments => "minecraft:stored_enchantments",
            Kind::DyedColor => "minecraft:dyed_color",
        }
    }

    /// The kind of the game's name `name`, if Clustine knows it.
    pub fn named(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// `levels` in the order of the enchantment registry, names it does not know last by
/// name: the one order an item's enchantments are kept in, whatever order they were
/// read in (an NBT compound's, a client's).
#[must_use]
pub fn in_registry_order(mut levels: Vec<(String, i32)>) -> Vec<(String, i32)> {
    let registry = clustine_data::synced_registry("minecraft:enchantment");
    let place = |name: &str| {
        registry
            .and_then(|registry| registry.entries.iter().position(|entry| *entry == name))
            .unwrap_or(usize::MAX)
    };
    levels.sort_by(|(a, _), (b, _)| place(a).cmp(&place(b)).then_with(|| a.cmp(b)));
    levels
}

/// One component's value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Component {
    PotionContents(PotionContents),
    /// How a potion's effects' durations are scaled where it is applied: 0.25 for a
    /// lingering potion, 0.125 for a tipped arrow, by the items' defaults.
    PotionDurationScale(Float),
    /// The stacks a crossbow is loaded with, at most 1,024.
    ChargedProjectiles(Vec<ItemStack>),
    /// Enchantments by name with their levels, in the registry's order
    /// ([`in_registry_order`]): the game keeps them in a map, whose order counts for
    /// nothing, so that two stacks of the same enchantments are the same kind.
    Enchantments(Vec<(String, i32)>),
    UseCooldown(UseCooldown),
    Fireworks(Fireworks),
    /// The wear an item has taken.
    Damage(i32),
    /// A marker: an arrow shot from it cannot be picked up (`intangible_projectile`).
    IntangibleProjectile,
    BannerPatterns(Vec<BannerLayer>),
    /// The item's name, a text component as the game's JSON writes it.
    ItemName(String),
    Rarity(Rarity),
    TooltipDisplay(TooltipDisplay),
    /// The effects a suspicious stew gives, each an effect by name and its duration in
    /// ticks.
    SuspiciousStewEffects(Vec<(String, i32)>),
    /// The amplifier of the bad omen an ominous bottle gives, 0 to 4.
    OminousBottleAmplifier(i32),
    /// The chicken variant an egg's chicks are, by name.
    ChickenVariant(String),
    /// The enchantments an enchanted book holds for an anvil to give, kept as
    /// [`Component::Enchantments`] is: by name with their levels, in the registry's order.
    /// The game's codecs of the two are one (`ItemEnchantments`).
    StoredEnchantments(Vec<(String, i32)>),
    /// A dyed leather item's colour (`DyedItemColor`): an RGB `int`, whose top byte the
    /// game's dyeing leaves 0. 26.3 keeps no tooltip flag in it (`tooltip_display` hides
    /// its line).
    DyedColor(i32),
}

impl Component {
    /// Its kind.
    pub fn kind(&self) -> Kind {
        match self {
            Component::PotionContents(_) => Kind::PotionContents,
            Component::PotionDurationScale(_) => Kind::PotionDurationScale,
            Component::ChargedProjectiles(_) => Kind::ChargedProjectiles,
            Component::Enchantments(_) => Kind::Enchantments,
            Component::UseCooldown(_) => Kind::UseCooldown,
            Component::Fireworks(_) => Kind::Fireworks,
            Component::Damage(_) => Kind::Damage,
            Component::IntangibleProjectile => Kind::IntangibleProjectile,
            Component::BannerPatterns(_) => Kind::BannerPatterns,
            Component::ItemName(_) => Kind::ItemName,
            Component::Rarity(_) => Kind::Rarity,
            Component::TooltipDisplay(_) => Kind::TooltipDisplay,
            Component::SuspiciousStewEffects(_) => Kind::SuspiciousStewEffects,
            Component::OminousBottleAmplifier(_) => Kind::OminousBottleAmplifier,
            Component::ChickenVariant(_) => Kind::ChickenVariant,
            Component::StoredEnchantments(_) => Kind::StoredEnchantments,
            Component::DyedColor(_) => Kind::DyedColor,
        }
    }
}

/// `PotionContents`: a potion by name, a colour of its own, effects of its own and a
/// name of its own for the translation, each optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PotionContents {
    pub potion: Option<String>,
    /// An RGB colour that overrides the effects' mix.
    pub custom_colour: Option<i32>,
    pub custom_effects: Vec<EffectInstance>,
    pub custom_name: Option<String>,
}

/// `MobEffectInstance` as an item carries it: an effect by name and its settings. The
/// game's `hidden_effect` (an instance kept under a stronger one) is left out: no item of
/// the creative tab or the loot N9 rolls carries one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectInstance {
    pub effect: String,
    pub amplifier: i32,
    /// In ticks; -1 for one without end.
    pub duration: i32,
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
}

/// `UseCooldown`: how long the item cannot be used again after a use, and the group of
/// items that share that wait (the item itself where none is named).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UseCooldown {
    pub seconds: Float,
    pub group: Option<String>,
}

/// `Fireworks`: a rocket's flight duration and its explosions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fireworks {
    pub flight_duration: i32,
    pub explosions: Vec<FireworkExplosion>,
}

/// `FireworkExplosion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FireworkExplosion {
    pub shape: FireworkShape,
    /// RGB colours.
    pub colours: Vec<i32>,
    pub fade_colours: Vec<i32>,
    pub trail: bool,
    pub twinkle: bool,
}

/// `FireworkExplosion.Shape`, in the game's order of ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FireworkShape {
    SmallBall,
    LargeBall,
    Star,
    Creeper,
    Burst,
}

/// One layer of `BannerPatternLayers`: a pattern by name and a dye colour by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BannerLayer {
    pub pattern: String,
    pub colour: String,
}

/// `Rarity`, in the game's order of ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
}

/// `TooltipDisplay`: whether the tooltip is hidden, and which components' lines are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TooltipDisplay {
    pub hide_tooltip: bool,
    /// Kinds by the game's name, kept as names so that one Clustine does not know stays.
    pub hidden_components: Vec<String>,
}

/// A stack's components beyond its item's defaults: none for a plain stack.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Components(pub Option<Box<Patch>>);

/// The game's `DataComponentPatch` over the kinds Clustine knows: the components added
/// or changed, one of a kind and in the order of [`Kind`], and the kinds removed, in the
/// same order. A kind is never in both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    pub added: Vec<Component>,
    pub removed: Vec<Kind>,
}

impl Components {
    /// None, as a plain stack has.
    pub const NONE: Components = Components(None);

    /// Whether the stack differs from its item's defaults in nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// The component of `kind` the patch adds or changes; `None` where it neither adds
    /// one nor removes the kind, and the item's default then holds.
    pub fn get(&self, kind: Kind) -> Option<&Component> {
        self.0
            .as_ref()?
            .added
            .iter()
            .find(|component| component.kind() == kind)
    }

    /// Whether the patch removes `kind`, which then reads as absent whatever the item's
    /// default.
    pub fn removes(&self, kind: Kind) -> bool {
        self.0
            .as_ref()
            .is_some_and(|patch| patch.removed.contains(&kind))
    }

    /// Sets `component`, in place of one of its kind or of its kind's removal.
    pub fn set(&mut self, component: Component) {
        let patch = self.0.get_or_insert_with(Box::default);
        let kind = component.kind();
        patch.removed.retain(|removed| *removed != kind);
        match patch.added.binary_search_by_key(&kind, Component::kind) {
            Ok(at) => patch.added[at] = component,
            Err(at) => patch.added.insert(at, component),
        }
    }

    /// Removes `kind`, so that it reads as absent.
    pub fn remove(&mut self, kind: Kind) {
        let patch = self.0.get_or_insert_with(Box::default);
        patch.added.retain(|component| component.kind() != kind);
        if let Err(at) = patch.removed.binary_search(&kind) {
            patch.removed.insert(at, kind);
        }
    }

    /// Forgets what the patch says of `kind`, so that the item's default holds again; a
    /// patch left with nothing becomes none.
    pub fn reset(&mut self, kind: Kind) {
        if let Some(patch) = &mut self.0 {
            patch.added.retain(|component| component.kind() != kind);
            patch.removed.retain(|removed| *removed != kind);
            if patch.added.is_empty() && patch.removed.is_empty() {
                self.0 = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_keeps_its_components_by_kind_whatever_the_order_they_are_set_in() {
        let mut one = Components::NONE;
        one.set(Component::Damage(3));
        one.set(Component::PotionDurationScale(Float(0.25)));
        let mut other = Components::NONE;
        other.set(Component::PotionDurationScale(Float(0.25)));
        other.set(Component::Damage(3));
        assert_eq!(one, other);
        assert_eq!(one.get(Kind::Damage), Some(&Component::Damage(3)));
    }

    #[test]
    fn a_kind_set_after_its_removal_is_no_longer_removed_and_the_reverse() {
        let mut components = Components::NONE;
        components.remove(Kind::Damage);
        assert!(components.removes(Kind::Damage));
        components.set(Component::Damage(1));
        assert!(!components.removes(Kind::Damage));
        components.remove(Kind::Damage);
        assert!(components.get(Kind::Damage).is_none());
        assert!(components.removes(Kind::Damage));
    }

    #[test]
    fn a_patch_reset_to_nothing_is_none_again() {
        let mut components = Components::NONE;
        components.set(Component::IntangibleProjectile);
        components.reset(Kind::IntangibleProjectile);
        assert!(components.is_empty());
    }

    #[test]
    fn every_kind_is_found_by_its_name() {
        for kind in Kind::ALL {
            assert_eq!(Kind::named(kind.name()), Some(kind));
        }
    }
}
