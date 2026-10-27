//! The throwables' use (`docs/groundwork/n9-plan.md`, section 4.4; part S4's): the
//! snowball, the eggs, the splash and lingering potions, the experience bottle and the
//! ender pearl (with its cooldown), each thrown from the player's eyes along their look
//! (`region/throws.rs`).
//!
//! Each `use` (n9 SnowballItem, EggItem, SplashPotionItem with ThrowablePotionItem,
//! LingeringPotionItem, ExperienceBottleItem, EnderpearlItem, lines 12–64 or 10–62 of
//! each): the throw's sound at the player's feet, volume 0.5, pitch `0.4 / (r · 0.4 +
//! 0.8)` of a draw of the level's random, before anything else; then
//! `spawnProjectileFromRotation` with the kind's roll, power and uncertainty; then
//! `consume(1)`, which a creative player is spared. The pearl's second of cooldown is no
//! rule of its item but its `use_cooldown` component, which `ItemStack.use` applies
//! after any use that lasts no time (`applyAfterUseComponentSideEffects`, n9 ItemStack
//! 517–620): the caller's.

use clustine_data::{entity_types, items, sounds};
use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind};

use crate::api::SOUND_SOURCE_PLAYERS;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::thrown;

/// How a kind of throwable is thrown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Throwable {
    /// The entity type it makes.
    pub(crate) kind: i32,
    /// The sound of the throw, and its source.
    pub(crate) sound: i32,
    pub(crate) source: i32,
    /// `spawnProjectileFromRotation`'s roll (`zRot`), power and uncertainty.
    pub(crate) roll: f32,
    pub(crate) power: f32,
    pub(crate) uncertainty: f32,
}

/// The throw's volume (every kind's 0.5).
pub(crate) const VOLUME: f32 = 0.5;

/// The pitch of a throw's sound, of a draw of the level's random.
pub(crate) fn pitch(drawn: f32) -> f32 {
    0.4 / (drawn * 0.4 + 0.8)
}

/// The throwable the item `item` is, if it is one.
pub(crate) fn of(item: i32) -> Option<Throwable> {
    // Egg, snowball and pearl: no roll, 1.5 and 1.0; the potions -20, 0.5 and 1.0
    // (`ThrowablePotionItem.PROJECTILE_SHOOT_POWER`); the bottle -20, 0.7 and 1.0.
    let thrown = |kind, sound, source| Throwable {
        kind,
        sound,
        source,
        roll: 0.0,
        power: thrown::PLAYER_POWER,
        uncertainty: thrown::PLAYER_UNCERTAINTY,
    };
    let lobbed = |kind, sound, source, power| Throwable {
        kind,
        sound,
        source,
        roll: -20.0,
        power,
        uncertainty: 1.0,
    };
    Some(match item {
        items::SNOWBALL => thrown(
            entity_types::SNOWBALL,
            sounds::ENTITY_SNOWBALL_THROW,
            SOUND_SOURCE_NEUTRAL,
        ),
        // The three eggs are one `EggItem`, and make the one `ThrownEgg`.
        items::EGG | items::BLUE_EGG | items::BROWN_EGG => thrown(
            entity_types::EGG,
            sounds::ENTITY_EGG_THROW,
            SOUND_SOURCE_PLAYERS,
        ),
        items::ENDER_PEARL => thrown(
            entity_types::ENDER_PEARL,
            sounds::ENTITY_ENDER_PEARL_THROW,
            SOUND_SOURCE_NEUTRAL,
        ),
        items::SPLASH_POTION => lobbed(
            entity_types::SPLASH_POTION,
            sounds::ENTITY_SPLASH_POTION_THROW,
            SOUND_SOURCE_PLAYERS,
            0.5,
        ),
        items::LINGERING_POTION => lobbed(
            entity_types::LINGERING_POTION,
            sounds::ENTITY_LINGERING_POTION_THROW,
            SOUND_SOURCE_NEUTRAL,
            0.5,
        ),
        items::EXPERIENCE_BOTTLE => lobbed(
            entity_types::EXPERIENCE_BOTTLE,
            sounds::ENTITY_EXPERIENCE_BOTTLE_THROW,
            SOUND_SOURCE_NEUTRAL,
            0.7,
        ),
        _ => return None,
    })
}

/// `UseCooldown.ticks()` of `stack`'s `use_cooldown` (its own, else its item's, unless
/// removed): `(int) (seconds · 20)`; `None` for a stack without one (n9 UseCooldown).
pub(crate) fn use_cooldown_ticks(stack: &ItemStack) -> Option<i32> {
    let seconds = match stack.components.get(Kind::UseCooldown) {
        Some(Component::UseCooldown(cooldown)) => cooldown.seconds.0,
        _ if stack.components.removes(Kind::UseCooldown) => return None,
        _ => {
            clustine_data::ItemComponents::of(stack.item)
                .use_cooldown?
                .seconds
        }
    };
    Some((seconds * 20.0) as i32)
}
