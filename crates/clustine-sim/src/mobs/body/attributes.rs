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

use super::{Attributes, Instance, Modifier};

/// `AttributeModifier.Operation`'s ordinals.
pub(crate) const ADD_VALUE: u8 = 0;
pub(crate) const ADD_MULTIPLIED_BASE: u8 = 1;
pub(crate) const ADD_MULTIPLIED_TOTAL: u8 = 2;

/// `Mob.finalizeSpawn`'s follow range bonus.
pub(crate) const RANDOM_SPAWN_BONUS: &str = "minecraft:random_spawn_bonus";

/// `SPEED_MODIFIER_POWDER_SNOW_ID`, powder snow's slowing.
pub(crate) const POWDER_SNOW: &str = "minecraft:powder_snow";

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
