//! The bow (`BowItem`; `docs/groundwork/n9-plan.md`, section 4.6; part S4's): the draw's
//! power by the ticks it was held, the arrow made at its release with the bow's
//! enchantments, the ammunition taken but by a creative player.
//!
//! Here too what the bow shares with the crossbow, `ProjectileWeaponItem`'s (n9
//! ProjectileWeaponItem): which stacks each fires, the draw of the ammunition into
//! projectiles (`draw` and `useAmmo`) and the angles a shot spreads them at.

use clustine_data::{entity_types, items};
use clustine_world::components::Component;
use clustine_world::{ItemStack, Pickup};

use super::enchantments;

/// What the power of the draw is multiplied by for the arrow's speed, and the shot's
/// uncertainty (`releaseUsing`'s `shoot(…, f · 3.0f, 1.0f, …)`, n9 BowItem 99–131).
pub(crate) const SPEED: f32 = 3.0;
pub(crate) const UNCERTAINTY: f32 = 1.0;

/// The least power a release looses an arrow at (`(double) f < 0.1`, n9 BowItem 54–65).
const LEAST_POWER: f64 = 0.1;

/// `BowItem.getPowerForTime(ticks)`: `f = t / 20; f = (f² + 2f) / 3`, at most 1, in
/// floats (n9 BowItem 134–158). Full at 20 ticks.
pub(crate) fn power_for_time(ticks: i32) -> f32 {
    let f = ticks as f32 / 20.0;
    let f = (f * f + f * 2.0) / 3.0;
    if f > 1.0 { 1.0 } else { f }
}

/// The power of a release after `ticks` held, or nothing where it is too weak to loose
/// an arrow.
pub(crate) fn release_power(ticks: i32) -> Option<f32> {
    let power = power_for_time(ticks);
    (f64::from(power) >= LEAST_POWER).then_some(power)
}

/// The pitch of the bow's `arrow_shoot` of `power`, of a draw of the level's random:
/// `1 / (r · 0.4 + 1.2) + f · 0.5` (n9 BowItem 134–181).
pub(crate) fn shoot_pitch(drawn: f32, power: f32) -> f32 {
    1.0 / (drawn * 0.4 + 1.2) + power * 0.5
}

/// `#minecraft:arrows` of the items (`ItemTags.ARROWS`; `tags/item/arrows.json`): what
/// a bow fires (`ARROW_ONLY`), and what a crossbow loads from the inventory.
pub(crate) fn is_arrow(item: i32) -> bool {
    matches!(
        item,
        items::ARROW | items::TIPPED_ARROW | items::SPECTRAL_ARROW
    )
}

/// What a weapon takes from a hand (`getSupportedHeldProjectiles`): the bow's arrows,
/// the crossbow's arrows and rockets (`ARROW_OR_FIREWORK`, n9 CrossbowItem
/// `getSupportedHeldProjectiles`).
pub(crate) fn fires_from_hand(weapon: i32, item: i32) -> bool {
    is_arrow(item) || (weapon == items::CROSSBOW && item == items::FIREWORK_ROCKET)
}

/// What a weapon takes from the inventory (`getAllSupportedProjectiles`): arrows, for
/// both.
pub(crate) fn fires_from_inventory(item: i32) -> bool {
    is_arrow(item)
}

/// The entity type an arrow item makes (`ProjectileWeaponItem.createProjectile`: the
/// ammunition's `ArrowItem`, else `Items.ARROW`'s; the tipped arrow is an `ArrowItem`
/// whose arrow is the plain one).
pub(crate) fn arrow_kind(ammo: i32) -> i32 {
    if ammo == items::SPECTRAL_ARROW {
        entity_types::SPECTRAL_ARROW
    } else {
        entity_types::ARROW
    }
}

/// `BowItem.use` and `CrossbowItem.use`'s start: whether a draw may begin, with
/// `projectile` what `Player.getProjectile` finds (a creative player finds an arrow
/// always, so may draw with none: n9 BowItem 170–196).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the start of a use (using.rs) is to ask it")
)]
pub(crate) fn can_draw(projectile: Option<&ItemStack>, infinite: bool) -> bool {
    infinite || projectile.is_some()
}

/// What `ProjectileWeaponItem.draw` leaves: the projectiles, and how many of the
/// ammunition stack it took (`split`), which the caller takes out of its slot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Drawn {
    pub(crate) projectiles: Vec<ItemStack>,
    pub(crate) taken: i32,
}

/// `ProjectileWeaponItem.draw(weapon, ammo, shooter)` (n9 ProjectileWeaponItem 193–254):
/// as many projectiles as Multishot makes; the first of `ammo` itself, the rest of a
/// copy made before it, each by `useAmmo` (256–317): of a copy, or by a shooter with
/// infinite materials, or where Infinity spares it, one of the stack marked
/// `intangible_projectile` and nothing taken; else as many as `processAmmoUse` says
/// split off the stack, none where it has too few.
pub(crate) fn draw(weapon: &ItemStack, ammo: &ItemStack, infinite: bool) -> Drawn {
    let mut drawn = Drawn {
        projectiles: Vec::new(),
        taken: 0,
    };
    if ammo.count <= 0 {
        return drawn;
    }
    let count = enchantments::projectile_count(weapon, 1);
    for index in 0..count {
        let copy = index > 0;
        let amount = if copy || infinite {
            0
        } else {
            enchantments::ammo_use(weapon, ammo, 1)
        };
        // The copy is made before the first is split off, so it has the whole count,
        // and takes nothing of it.
        if amount > ammo.count {
            continue;
        }
        if amount == 0 {
            let mut one = ammo.with_count(1);
            one.components.set(Component::IntangibleProjectile);
            drawn.projectiles.push(one);
        } else {
            drawn.projectiles.push(ammo.with_count(amount));
            drawn.taken += amount;
        }
    }
    drawn
}

/// `ProjectileWeaponItem.shoot`'s angles for `count` projectiles spread over `spread`
/// degrees (n9 ProjectileWeaponItem 49–200): `step = 2 · spread / (n − 1)` (0 for one),
/// `offset = ((n − 1) % 2) · step / 2`, and the i-th at `offset + sign · ((i + 1) / 2) ·
/// step`, the sign turning at each: 0, −10, +10 for Multishot's three.
pub(crate) fn angles(count: usize, spread: f32) -> Vec<f32> {
    let step = if count == 1 {
        0.0
    } else {
        2.0 * spread / (count as f32 - 1.0)
    };
    let offset = ((count as i32 - 1) % 2) as f32 * step / 2.0;
    let mut sign = 1.0_f32;
    (0..count)
        .map(|index| {
            let angle = offset + sign * ((index as i32 + 1) / 2) as f32 * step;
            sign = -sign;
            angle
        })
        .collect()
}

/// `AbstractArrow`'s pickup of an arrow of the projectile stack `projectile`: one of
/// intangible ammunition only by a creative player (its constructor), else any player's
/// arrow by anyone (`setOwner` of a player, n9 AbstractArrow 1709–1749).
pub(crate) fn player_arrow_pickup(projectile: &ItemStack) -> Pickup {
    let intangible = projectile
        .components
        .get(clustine_world::components::Kind::IntangibleProjectile)
        .is_some();
    if intangible {
        Pickup::CreativeOnly
    } else {
        Pickup::Allowed
    }
}
