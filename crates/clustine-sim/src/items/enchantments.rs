//! The enchantments N9's rules read (`docs/groundwork/n9-plan.md`, section 4.10; part
//! S4's): each a rule written from its data file's values (Power, Punch, Flame, Infinity,
//! Multishot, Piercing, Quick Charge, Loyalty, Impaling, and Unbreaking and Mending for
//! wear), read from a stack's `enchantments` component. The reads below are what the
//! arrows (S2) and the weapons (S4) call.
//!
//! 26.3's enchantments are data (`data/minecraft/enchantment/*.json` of the generated
//! data), whose effects the game runs through `EnchantmentHelper` (n9
//! EnchantmentHelper): each effect is a `LevelBasedValue`, here only the linear kind,
//! `base + per_level_above_first · (level − 1)` in floats, added to (or set in place of)
//! the value the rule starts from, and the sum floored at 0 where the helper floors it
//! (`processProjectileCount` 608–627, `processProjectileSpread` 629–647,
//! `getPiercingCount` 649–666, `getTridentReturnToOwnerAcceleration` 836–854 and
//! `modifyCrossbowChargingTime` 856–872 each end in `Math.max(0, …)`). The values are
//! written here by hand, each with its file, rather than read from the data at run time:
//! the simulation reads no files.
//!
//! Named differences: Riptide's spin and Channeling's lightning are not built (decision
//! 1 of the plan); Riptide is read only so that a trident that has it is not thrown.
//! Unbreaking's `remove_binomial` is not in the dumps: it is taken as a draw of the
//! level's random per point of wear, each kept off where the draw is below the chance
//! (**Inferred**).

use clustine_data::{ItemComponents, TAGS, items};
use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind};

/// The level of the enchantment `name` (`minecraft:power`, say) on `stack`, 0 where it
/// has none (`EnchantmentHelper.getItemEnchantmentLevel`). An item's defaults hold no
/// enchantments, so only the patch is read.
pub(crate) fn level(stack: &ItemStack, name: &str) -> i32 {
    match stack.components.get(Kind::Enchantments) {
        Some(Component::Enchantments(enchantments)) => enchantments
            .iter()
            .find(|(enchantment, _)| enchantment == name)
            .map_or(0, |(_, level)| *level),
        _ => 0,
    }
}

/// `LevelBasedValue.Linear.calculate(level)`: `base + per_level_above_first · (level −
/// 1)`, in floats.
fn linear(base: f32, per_level_above_first: f32, level: i32) -> f32 {
    base + per_level_above_first * (level - 1) as f32
}

/// The value of an `add` effect of the enchantment `name` on `stack` at its level, or
/// nothing where the stack has none of it.
fn added(stack: &ItemStack, name: &str, base: f32, per_level_above_first: f32) -> f32 {
    match level(stack, name) {
        0 => 0.0,
        level => linear(base, per_level_above_first, level),
    }
}

/// Whether the tag `name` of the registry `registry` holds the id `id`, as the generated
/// tags have it.
fn tagged(registry: &str, name: &str, id: i32) -> bool {
    TAGS.iter()
        .find(|tags| tags.registry == registry)
        .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
        .is_some_and(|tag| tag.entries.contains(&id))
}

/// `#minecraft:arrows` of the entity types: the arrow and the spectral arrow, the
/// projectiles Power and Punch are for (their `direct_attacker` requirement).
pub(crate) fn is_arrow_entity(kind: i32) -> bool {
    tagged("minecraft:entity_type", "minecraft:arrows", kind)
}

/// Power (`power.json`): `minecraft:damage`, `add` of `linear(1.0, 0.5)`, where the
/// direct attacker is in `#minecraft:arrows`. `damage` raised by it for an arrow of the
/// entity type `direct` shot from `weapon` (`EnchantmentHelper.modifyDamage`).
#[cfg_attr(not(test), expect(dead_code, reason = "the arrows' hit (S2) reads it"))]
pub(crate) fn power_damage(weapon: &ItemStack, direct: i32, damage: f32) -> f32 {
    if !is_arrow_entity(direct) {
        return damage;
    }
    damage + added(weapon, "minecraft:power", 1.0, 0.5)
}

/// Punch (`punch.json`): `minecraft:knockback`, `add` of `linear(1.0, 1.0)`, where the
/// direct attacker is in `#minecraft:arrows` (`EnchantmentHelper.modifyKnockback`).
#[cfg_attr(not(test), expect(dead_code, reason = "the arrows' hit (S2) reads it"))]
pub(crate) fn punch_knockback(weapon: &ItemStack, direct: i32, knockback: f32) -> f32 {
    if !is_arrow_entity(direct) {
        return knockback;
    }
    knockback + added(weapon, "minecraft:punch", 1.0, 1.0)
}

/// Flame (`flame.json`): `minecraft:projectile_spawned`, `ignite` for 100 seconds: the
/// ticks of fire a projectile shot from `weapon` is set alight for as it is added
/// (`Projectile.applyOnProjectileSpawned`; `Entity.igniteForSeconds`, `Mth.floor(100 ·
/// 20)`), 0 without it.
pub(crate) fn projectile_fire_ticks(weapon: &ItemStack) -> i32 {
    const SECONDS: f32 = 100.0;
    if level(weapon, "minecraft:flame") > 0 {
        (SECONDS * 20.0).floor() as i32
    } else {
        0
    }
}

/// Infinity (`infinity.json`): `minecraft:ammo_use`, `set` to 0 where the ammunition is
/// `minecraft:arrow` (its `match_tool` requirement): how many of `ammo` a shot from
/// `weapon` takes, of `amount` (`EnchantmentHelper.processAmmoUse`).
pub(crate) fn ammo_use(weapon: &ItemStack, ammo: &ItemStack, amount: i32) -> i32 {
    if level(weapon, "minecraft:infinity") > 0 && ammo.item == items::ARROW {
        0
    } else {
        amount
    }
}

/// Multishot (`multishot.json`): `minecraft:projectile_count`, `add` of `linear(2.0,
/// 2.0)`: how many projectiles a draw of `weapon` makes of `count`, at least none
/// (`EnchantmentHelper.processProjectileCount`; the float's `intValue`).
pub(crate) fn projectile_count(weapon: &ItemStack, count: i32) -> i32 {
    ((count as f32 + added(weapon, "minecraft:multishot", 2.0, 2.0)) as i32).max(0)
}

/// Multishot: `minecraft:projectile_spread`, `add` of `linear(10.0, 10.0)`: the degrees
/// the projectiles of a shot from `weapon` spread over, of `spread`
/// (`EnchantmentHelper.processProjectileSpread`).
pub(crate) fn projectile_spread(weapon: &ItemStack, spread: f32) -> f32 {
    (spread + added(weapon, "minecraft:multishot", 10.0, 10.0)).max(0.0)
}

/// Piercing (`piercing.json`): `minecraft:projectile_piercing`, `add` of `linear(1.0,
/// 1.0)`: how many entities an arrow shot from `weapon` passes through
/// (`EnchantmentHelper.getPiercingCount`, which `AbstractArrow`'s constructor sets as its
/// pierce level where it is above 0).
pub(crate) fn piercing(weapon: &ItemStack) -> i32 {
    (added(weapon, "minecraft:piercing", 1.0, 1.0) as i32).max(0)
}

/// Quick Charge (`quick_charge.json`): `minecraft:crossbow_charge_time`, `add` of
/// `linear(-0.25, -0.25)`: the seconds a crossbow's charge takes of `seconds`, at least
/// none (`EnchantmentHelper.modifyCrossbowChargingTime`).
pub(crate) fn crossbow_charge_seconds(crossbow: &ItemStack, seconds: f32) -> f32 {
    (seconds + added(crossbow, "minecraft:quick_charge", -0.25, -0.25)).max(0.0)
}

/// Quick Charge's `minecraft:crossbow_charging_sounds`, one a level: the sound its
/// charge starts with at each level (`quick_charge_1` to `_3`), none at the middle, the
/// usual end. `None` without Quick Charge (`EnchantmentHelper.pickHighestLevel`: the
/// list's entry at the level, the last past its end).
pub(crate) fn quick_charge_start_sound(crossbow: &ItemStack) -> Option<i32> {
    use clustine_data::sounds;
    const STARTS: [i32; 3] = [
        sounds::ITEM_CROSSBOW_QUICK_CHARGE_1,
        sounds::ITEM_CROSSBOW_QUICK_CHARGE_2,
        sounds::ITEM_CROSSBOW_QUICK_CHARGE_3,
    ];
    match level(crossbow, "minecraft:quick_charge") {
        level if level <= 0 => None,
        level => Some(STARTS[(level.min(3) - 1) as usize]),
    }
}

/// Loyalty (`loyalty.json`): `minecraft:trident_return_acceleration`, `add` of
/// `linear(1.0, 1.0)`: how hard a thrown trident of the pickup stack `trident` pulls
/// back to its owner (`EnchantmentHelper.getTridentReturnToOwnerAcceleration`).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the trident's tick (S2) reads it")
)]
pub(crate) fn loyalty(trident: &ItemStack) -> i32 {
    (added(trident, "minecraft:loyalty", 1.0, 1.0) as i32).max(0)
}

/// Impaling (`impaling.json`): `minecraft:damage`, `add` of `linear(2.5, 2.5)`, where
/// the entity hurt is in `#minecraft:sensitive_to_impaling` (which is `#aquatic`):
/// `damage` raised by it for a hit with `weapon` (a trident, thrown or held) on an
/// entity of the type `target`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the trident's hit (S2) reads it")
)]
pub(crate) fn impaling_damage(weapon: &ItemStack, target: i32, damage: f32) -> f32 {
    if !tagged(
        "minecraft:entity_type",
        "minecraft:sensitive_to_impaling",
        target,
    ) {
        return damage;
    }
    damage + added(weapon, "minecraft:impaling", 2.5, 2.5)
}

/// Riptide (`riptide.json`): `minecraft:trident_spin_attack_strength`, `add` of
/// `linear(1.5, 0.75)` (`EnchantmentHelper.getTridentSpinAttackStrength`, not floored).
pub(crate) fn riptide_strength(trident: &ItemStack) -> f32 {
    added(trident, "minecraft:riptide", 1.5, 0.75)
}

/// Unbreaking (`unbreaking.json`): `minecraft:item_damage`, `remove_binomial` with the
/// chance `linear(2, 2) / linear(10, 5)` for what is in `#minecraft:enchantable/armor`
/// and `linear(1, 1) / linear(2, 1)` for anything else: of `amount` points of wear, each
/// kept off where a draw of `next_float` is below the chance
/// (`EnchantmentHelper.processDurabilityChange`; the binomial **Inferred**). No draw
/// without Unbreaking.
pub(crate) fn durability_change(
    stack: &ItemStack,
    amount: i32,
    mut next_float: impl FnMut() -> f32,
) -> i32 {
    let unbreaking = level(stack, "minecraft:unbreaking");
    if unbreaking <= 0 {
        return amount;
    }
    let armour = tagged("minecraft:item", "minecraft:enchantable/armor", stack.item);
    let chance = if armour {
        linear(2.0, 2.0, unbreaking) / linear(10.0, 5.0, unbreaking)
    } else {
        linear(1.0, 1.0, unbreaking) / linear(2.0, 1.0, unbreaking)
    };
    let kept_off = (0..amount).filter(|_| next_float() < chance).count();
    amount - i32::try_from(kept_off).expect("no more than the amount")
}

/// The wear of `stack` (`ItemStack.getDamageValue`: its `damage`, 0 without any).
pub(crate) fn damage(stack: &ItemStack) -> i32 {
    match stack.components.get(Kind::Damage) {
        Some(Component::Damage(damage)) => *damage,
        _ => 0,
    }
}

/// `ItemStack.nextDamageWillBreak`: it wears, and a point more would wear it out.
pub(crate) fn next_damage_will_break(stack: &ItemStack) -> bool {
    ItemComponents::of(stack.item)
        .max_damage
        .is_some_and(|max| damage(stack) >= i32::from(max) - 1)
}

/// `ItemStack.applyDamage` of `change` more wear (after `durability_change`): the wear
/// set, at most the item's most, and the stack one less where that wears it out
/// (`hurtAndBreak`); `None` where nothing is left. A stack that does not wear is as it
/// was.
pub(crate) fn hurt_and_break(stack: &ItemStack, change: i32) -> Option<ItemStack> {
    let Some(max) = ItemComponents::of(stack.item).max_damage.map(i32::from) else {
        return Some(stack.clone());
    };
    if change == 0 {
        return Some(stack.clone());
    }
    let mut worn = stack.clone();
    let damage = (damage(stack) + change).clamp(0, max);
    worn.components.set(Component::Damage(damage));
    if damage >= max {
        worn.count -= 1;
    }
    (worn.count > 0).then_some(worn)
}

/// `ItemStack.hurtWithoutBreaking` of `change` more wear: as [`hurt_and_break`], but at
/// most one short of wearing it out.
pub(crate) fn hurt_without_breaking(stack: &ItemStack, change: i32) -> ItemStack {
    let Some(max) = ItemComponents::of(stack.item).max_damage.map(i32::from) else {
        return stack.clone();
    };
    if change == 0 {
        return stack.clone();
    }
    let mut worn = stack.clone();
    let damage = (damage(stack) + change).min(max - 1).clamp(0, max);
    worn.components.set(Component::Damage(damage));
    worn
}

/// Mending (`mending.json`): `minecraft:repair_with_xp`, `multiply` by 2: the wear
/// `experience` points of an orb mend on `stack` (`EnchantmentHelper.
/// modifyDurabilityToRepairFromXp`, the float's `intValue`); none without Mending, which
/// is what picks the stack to mend.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the orbs' pick-up (S3) reads it")
)]
pub(crate) fn mending_repair(stack: &ItemStack, experience: i32) -> i32 {
    if level(stack, "minecraft:mending") > 0 {
        (experience as f32 * 2.0) as i32
    } else {
        0
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
