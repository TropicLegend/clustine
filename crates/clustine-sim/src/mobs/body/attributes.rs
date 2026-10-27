//! A mob's attributes (`AttributeMap`, `AttributeInstance`; the brief's section 1.2): its
//! type's defaults (`DefaultAttributes`, extracted whole as `DEFAULT_ATTRIBUTES`), and
//! the instances the mob has made of them, with their own base values and modifiers.
//!
//! The game makes an instance the first time something asks for it by `getAttribute`,
//! and what a client is sent depends on which exist (finding 17): those the record keeps
//! are the ones made, but `movement_speed`, which every mob makes in its first tick
//! (`aiStep` → `removeFrost`), is taken as made from then on without being written, so
//! that a mob at rest commits nothing for it ([`Attributes::syncable`]).
//!
//! A value is `calculateValue`'s, the modifiers of each operation in the order of their
//! ids where the game's is a hash map's, which cannot change the result for the one or
//! two of an operation that a mob of N7 has (the brief's section 1.2, **Inferred**).

use clustine_data::{ATTRIBUTES, DEFAULT_ATTRIBUTES, attributes as ids};

use super::{Attributes, Equipment, Instance, Modifier};

/// `AttributeModifier.Operation`'s ordinals.
pub(crate) const ADD_VALUE: u8 = 0;
pub(crate) const ADD_MULTIPLIED_BASE: u8 = 1;
pub(crate) const ADD_MULTIPLIED_TOTAL: u8 = 2;

/// `Mob.finalizeSpawn`'s follow range bonus.
pub(crate) const RANDOM_SPAWN_BONUS: &str = "minecraft:random_spawn_bonus";

/// `SPEED_MODIFIER_POWDER_SNOW_ID`, powder snow's slowing.
pub(crate) const POWDER_SNOW: &str = "minecraft:powder_snow";

/// Swiftness's speed (`MobEffects.SPEED`'s modifier, `effect.speed`).
pub(crate) const SPEED_EFFECT: &str = "minecraft:effect.speed";

/// What the type of the entity type `kind` gives the attribute `attribute`, if it has
/// it (`AttributeSupplier`).
pub(crate) fn supplied(kind: u16, attribute: u16) -> Option<f64> {
    let defaults = DEFAULT_ATTRIBUTES
        .get(usize::from(kind))
        .copied()
        .flatten()?;
    defaults
        .binary_search_by_key(&attribute, |&(id, _)| id)
        .ok()
        .map(|at| defaults[at].1)
}

/// The modifiers of `attribute` that what a mob wears and holds gives it: each item's
/// `attribute_modifiers` (its default: a stack's own patch of them is not read) whose
/// slot group takes in the slot it is in, by `EquipmentSlotGroup`'s ordinal.
pub(crate) fn worn_modifiers(equipment: &Equipment, attribute: u16) -> Vec<Modifier> {
    let mut worn = Vec::new();
    for (slot, stack) in equipment.slots.iter().enumerate() {
        let Some(stack) = stack else {
            continue;
        };
        for modifier in clustine_data::ItemComponents::of(stack.item).modifiers {
            let applies = match modifier.slot {
                0 => true,
                1 => slot == 0,
                2 => slot == 1,
                3 => slot <= 1,
                // The feet to the head, at the slots 2 to 5.
                group @ 4..=7 => slot == usize::from(group) - 2,
                8 => (2..=6).contains(&slot),
                9 => slot == 6,
                10 => slot == 7,
                _ => false,
            };
            if applies && modifier.attribute == attribute {
                worn.push(Modifier {
                    id: modifier.id.to_owned(),
                    amount: modifier.amount,
                    operation: modifier.operation,
                });
            }
        }
    }
    worn
}

/// `RangedAttribute.sanitizeValue`: within the attribute's bounds, its least for NaN.
fn sanitize(attribute: u16, value: f64) -> f64 {
    let range = &ATTRIBUTES[usize::from(attribute)];
    if value.is_nan() {
        return range.min;
    }
    value.clamp(range.min, range.max)
}

impl Instance {
    /// A new instance of `attribute` at `base`, without modifiers.
    fn new(attribute: u16, base: f64) -> Self {
        Self {
            attribute,
            base,
            modifiers: Vec::new(),
        }
    }

    /// `calculateValue()`: the base with every `add_value`, then the base times every
    /// `add_multiplied_base`, then all of it times one more than every
    /// `add_multiplied_total`, kept within the attribute's bounds.
    pub(crate) fn value(&self) -> f64 {
        let mut base = self.base;
        for modifier in self.of(ADD_VALUE) {
            base += modifier.amount;
        }
        let mut total = base;
        for modifier in self.of(ADD_MULTIPLIED_BASE) {
            total += base * modifier.amount;
        }
        for modifier in self.of(ADD_MULTIPLIED_TOTAL) {
            total *= 1.0 + modifier.amount;
        }
        sanitize(self.attribute, total)
    }

    fn of(&self, operation: u8) -> impl Iterator<Item = &Modifier> {
        self.modifiers
            .iter()
            .filter(move |modifier| modifier.operation == operation)
    }

    pub(crate) fn has_modifier(&self, id: &str) -> bool {
        self.modifiers.iter().any(|modifier| modifier.id == id)
    }

    /// `addPermanentModifier` (and `addTransientModifier`, which the record keeps all
    /// the same): a modifier of an id the instance has is not added (the game throws).
    pub(crate) fn add_modifier(&mut self, modifier: Modifier) {
        match self
            .modifiers
            .binary_search_by(|held| held.id.as_str().cmp(&modifier.id))
        {
            Ok(_) => {}
            Err(at) => self.modifiers.insert(at, modifier),
        }
    }

    /// `removeModifier(id)`: whether it had it.
    pub(crate) fn remove_modifier(&mut self, id: &str) -> bool {
        let before = self.modifiers.len();
        self.modifiers.retain(|modifier| modifier.id != id);
        self.modifiers.len() != before
    }
}

impl Attributes {
    /// The instance of `attribute` the mob has made, if any.
    pub(crate) fn made(&self, attribute: u16) -> Option<&Instance> {
        self.instances
            .binary_search_by_key(&attribute, |instance| instance.attribute)
            .ok()
            .map(|at| &self.instances[at])
    }

    /// `getAttributeValue(attribute)` of a mob of the type `kind`: its instance's value
    /// where it made one, else its type's (which the game reads without making one).
    /// An attribute its type does not have, which the game's code never asks of it, is
    /// the attribute's default.
    pub(crate) fn value(&self, kind: u16, attribute: u16) -> f64 {
        match self.made(attribute) {
            Some(instance) => instance.value(),
            None => supplied(kind, attribute).unwrap_or(ATTRIBUTES[usize::from(attribute)].default),
        }
    }

    /// `getAttributeValue(attribute)` of a mob of the type `kind` that wears and holds
    /// `equipment`: its own modifiers and those of what it wears and holds, which the
    /// game adds as transient modifiers when its equipment changes
    /// (`collectEquipmentChanges`) and Clustine reads from the equipment wherever it asks.
    pub(crate) fn value_worn(&self, kind: u16, attribute: u16, equipment: &Equipment) -> f64 {
        let worn = worn_modifiers(equipment, attribute);
        if worn.is_empty() {
            return self.value(kind, attribute);
        }
        let mut instance = self.made(attribute).cloned().unwrap_or_else(|| {
            let base =
                supplied(kind, attribute).unwrap_or(ATTRIBUTES[usize::from(attribute)].default);
            Instance::new(attribute, base)
        });
        for modifier in worn {
            instance.add_modifier(modifier);
        }
        instance.value()
    }

    /// `getAttribute(attribute)`: the mob's instance, made from its type's where it has
    /// none yet; `None` for an attribute its type does not have.
    pub(crate) fn instance(&mut self, kind: u16, attribute: u16) -> Option<&mut Instance> {
        let at = match self
            .instances
            .binary_search_by_key(&attribute, |instance| instance.attribute)
        {
            Ok(at) => at,
            Err(at) => {
                let base = supplied(kind, attribute)?;
                self.instances.insert(at, Instance::new(attribute, base));
                at
            }
        };
        Some(&mut self.instances[at])
    }

    /// The instances a client is sent at pairing (`getSyncableAttributes`): the made
    /// ones whose attribute is synced, and `movement_speed` from the mob's first tick
    /// on, made then by every mob, as each was made where the mob did not make it
    /// itself (its type's value, no modifier). Ascending by attribute.
    pub(crate) fn syncable(&self, kind: u16, ticked: bool) -> Vec<Instance> {
        let mut found: Vec<Instance> = self
            .instances
            .iter()
            .filter(|instance| ATTRIBUTES[usize::from(instance.attribute)].syncable)
            .cloned()
            .collect();
        let speed = ids::MOVEMENT_SPEED as u16;
        if ticked
            && self.made(speed).is_none()
            && let Some(base) = supplied(kind, speed)
        {
            let at = found
                .binary_search_by_key(&speed, |instance| instance.attribute)
                .unwrap_or_else(|at| at);
            found.insert(at, Instance::new(speed, base));
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::{entity_types, items};
    use clustine_world::ItemStack;

    use super::*;
    use crate::living::slots;

    fn zombie() -> u16 {
        u16::try_from(entity_types::ZOMBIE).expect("ids fit")
    }

    fn wearing(slot: u8, item: i32) -> Equipment {
        let mut equipment = Equipment::default();
        equipment.slots[usize::from(slot)] = Some(ItemStack::new(item, 1));
        equipment
    }

    #[test]
    fn a_sword_in_the_main_hand_adds_its_damage_and_in_the_off_hand_nothing() {
        let attributes = Attributes::default();
        let damage = u16::try_from(ids::ATTACK_DAMAGE).expect("ids fit");
        let bare = attributes.value(zombie(), damage);
        let held = wearing(slots::MAIN_HAND, items::IRON_SWORD);
        // An iron sword's `base_attack_damage` adds 5.
        assert_eq!(attributes.value_worn(zombie(), damage, &held), bare + 5.0);
        let off = wearing(slots::OFF_HAND, items::IRON_SWORD);
        assert_eq!(attributes.value_worn(zombie(), damage, &off), bare);
    }

    #[test]
    fn a_chestplate_worn_adds_its_armour_and_held_adds_none() {
        let attributes = Attributes::default();
        let armour = u16::try_from(ids::ARMOR).expect("ids fit");
        let bare = attributes.value(zombie(), armour);
        let worn = wearing(slots::CHEST, items::IRON_CHESTPLATE);
        assert_eq!(attributes.value_worn(zombie(), armour, &worn), bare + 6.0);
        let held = wearing(slots::MAIN_HAND, items::IRON_CHESTPLATE);
        assert_eq!(attributes.value_worn(zombie(), armour, &held), bare);
    }
}
