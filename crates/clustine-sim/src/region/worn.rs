//! What a player's equipment adds to their attributes (V2, `docs/groundwork/v-plan.md`,
//! section 4.5): `LivingEntity.collectEquipmentChanges` @112-231 takes each slot's stack,
//! unless it is empty or broken (`ItemStack.isBroken`), and adds its modifiers for that
//! slot (`ItemStack.forEachModifier`): its item's `attribute_modifiers` and its
//! enchantments' `minecraft:attributes` effects (`EnchantmentHelper.forEachModifier`;
//! `EnchantmentAttributeEffect.getModifier`, whose id is the effect's with `/` and the
//! slot's name, `idForSlot`). Efficiency's `mining_efficiency`, Aqua Affinity's
//! `submerged_mining_speed`, armour's points and toughness are among them.
//!
//! The client predicts by its own attributes (an Efficiency pickaxe digs faster, armour
//! fills its bar), so what changes of the syncable ones is told to it after the inputs,
//! with the player's other parts of the tick (`detectEquipmentUpdates` in
//! `LivingEntity.tick`, sent with the entity's dirty attributes), and every one again when
//! the player enters the world, as their effects are. The region reads the values it
//! keeps from that last look, as the game's attribute map holds them between two.
//!
//! Named differences: a stack's own patch of `attribute_modifiers` is not read (the
//! item's default is; no stack of Clustine's carries one), nor `unbreakable` for
//! `isBroken`; the others who see the player are not told these attributes (the game's
//! `broadcastAndSend`), none of which shows of a player to another; and after a hand-over
//! to another region they are told the player's client once more.

use clustine_data::enchanting::enchantment;
use clustine_data::{ATTRIBUTES, EQUIPMENT_SLOTS, ItemComponents, slot_group_holds};
use clustine_world::components::{Component, Kind};
use clustine_world::{ItemStack, PlayerId};

use super::Region;
use crate::api::{PlayerEvent, TickOutput};

/// One modifier a player's equipment gives: the attribute by its id, the modifier's id,
/// its amount and `AttributeModifier.Operation`'s ordinal.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct WornModifier {
    pub(super) attribute: u16,
    pub(super) id: String,
    pub(super) amount: f64,
    pub(super) operation: u8,
}

/// The modifiers a player's equipment gave at the last look, in memory only: a restore
/// or a hand-over looks again.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Worn(pub(super) Vec<WornModifier>);

/// `ItemStack.isBroken`: damageable and worn to its most.
fn is_broken(stack: &ItemStack) -> bool {
    let Some(most) = ItemComponents::of(stack.item).max_damage else {
        return false;
    };
    let damage = match stack.components.get(Kind::Damage) {
        Some(Component::Damage(damage)) => *damage,
        _ => 0,
    };
    damage >= i32::from(most)
}

/// The modifiers `equipment` (by `EquipmentSlot` ordinal) gives, slot by slot: the item's
/// own, then its enchantments' in the stack's order. A second modifier of an id the
/// attribute already has is left out, as `addTransientModifier` refuses one.
pub(super) fn worn_modifiers(equipment: &[(u8, Option<ItemStack>)]) -> Vec<WornModifier> {
    let mut worn: Vec<WornModifier> = Vec::new();
    let mut add = |modifier: WornModifier| {
        if !worn
            .iter()
            .any(|held| held.attribute == modifier.attribute && held.id == modifier.id)
        {
            worn.push(modifier);
        }
    };
    for (slot, stack) in equipment {
        let Some(stack) = stack else {
            continue;
        };
        if is_broken(stack) {
            continue;
        }
        for modifier in ItemComponents::of(stack.item).modifiers {
            if slot_group_holds(modifier.slot, *slot) {
                add(WornModifier {
                    attribute: modifier.attribute,
                    id: modifier.id.to_owned(),
                    amount: modifier.amount,
                    operation: modifier.operation,
                });
            }
        }
        let Some(Component::Enchantments(levels)) = stack.components.get(Kind::Enchantments) else {
            continue;
        };
        let Some(slot_name) = EQUIPMENT_SLOTS.get(usize::from(*slot)) else {
            continue;
        };
        for (name, level) in levels {
            let Some(enchantment) = enchantment(name) else {
                continue;
            };
            if !enchantment.matching_slot(*slot) {
                continue;
            }
            for effect in enchantment.attributes {
                add(WornModifier {
                    attribute: effect.attribute,
                    id: format!("{}/{slot_name}", effect.id),
                    amount: f64::from(effect.amount.calculate(*level)),
                    operation: effect.operation,
                });
            }
        }
    }
    worn
}

/// Whether a client is sent the attribute `attribute` (`Attribute.isClientSyncable`).
pub(super) fn syncable(attribute: u16) -> bool {
    ATTRIBUTES
        .get(usize::from(attribute))
        .is_some_and(|attribute| attribute.syncable)
}

impl Region {
    /// The players' look at their equipment after the inputs: whoever's modifiers
    /// changed is told the syncable attributes they touch, whole.
    pub(super) fn watch_worn(&mut self, output: &mut TickOutput) {
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            let now = worn_modifiers(&player.equipment());
            if now == player.worn.0 {
                continue;
            }
            let mut touched: Vec<u16> = player
                .worn
                .0
                .iter()
                .chain(&now)
                .map(|modifier| modifier.attribute)
                .filter(|&attribute| syncable(attribute))
                .collect();
            touched.sort_unstable();
            touched.dedup();
            player.worn = Worn(now);
            if touched.is_empty() {
                continue;
            }
            let attributes = touched
                .into_iter()
                .map(|attribute| player.attribute_shown(i32::from(attribute)))
                .collect();
            output
                .player_events
                .push((id, PlayerEvent::Attributes(attributes)));
        }
    }

    /// The value of the player `id`'s attribute `attribute` as their attribute map holds
    /// it (`getAttributeValue`): their base, their effects' modifiers and their
    /// equipment's at the last look; the attribute's default for a player not here.
    pub(crate) fn player_attribute(&self, id: PlayerId, attribute: i32) -> f64 {
        let Some(player) = self.players.get(&id) else {
            return usize::try_from(attribute)
                .ok()
                .and_then(|at| ATTRIBUTES.get(at))
                .map_or(0.0, |attribute| attribute.default);
        };
        let shown = player.attribute_shown(attribute);
        crate::mobs::body::Instance {
            attribute: shown.attribute,
            base: shown.base,
            modifiers: shown
                .modifiers
                .into_iter()
                .map(|(id, amount, operation)| crate::mobs::body::Modifier {
                    id,
                    amount,
                    operation,
                })
                .collect(),
        }
        .value()
    }
}
