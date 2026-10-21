//! The making of a mob, the same for an egg and for generation
//! (`docs/groundwork/n7l-plan.md`, sections 2.1 and 9 item 11): its own draws at its
//! construction, its health, air and head, and `finalizeSpawn`, the type's, `Mob`'s,
//! then what the type does after its parent's. Free of any region: the egg hands it a
//! player's stream aside as the level's random, generation the run's or the spawn's.

use clustine_data::attributes as ids;
use clustine_world::generation::Draws;
use uuid::Uuid;

use super::Mob;
use super::body::attributes::{ADD_MULTIPLIED_BASE, RANDOM_SPAWN_BONUS, supplied};
use super::body::{Head, Modifier, attribute};
use super::types::{self, Spawning};
use crate::region::Random;

/// A mob as `EntityType.create` makes it: its UUID and its constructor's yaw drawn from
/// `own`, its brain made (`construct`, its sensors' delays) and the type's constructor's
/// own draws, of its type's full health and air, its head and body turned to `yaw`.
pub(crate) fn construct(kind: u16, own: &mut Random, yaw: f32) -> (Uuid, Mob) {
    let uuid = crate::region::insecure_uuid(own.next_u64(), own.next_u64());
    let _constructor_yaw = own.next_float();
    let mob_type = types::of(kind);
    let mut mob = Mob::fresh();
    crate::mobs::brain::construct(mob_type, &mut mob, own);
    // `setHealth(getMaxHealth())` in the constructor, of its type's attributes.
    mob.body.vitals.health = supplied(kind, attribute(ids::MAX_HEALTH)).unwrap_or(20.0) as f32;
    mob.body.vitals.air = mob_type.max_air();
    mob.body.head = Head {
        head_yaw: yaw,
        body_yaw: yaw,
        still_since: 0,
        ..Head::default()
    };
    (uuid, mob)
}

/// `finalizeSpawn` of a mob of the type `kind`: its type's, `Mob`'s own part, then what
/// its type does after its parent's.
pub(crate) fn finalize(kind: u16, spawning: &mut Spawning<'_>) {
    let mob_type = types::of(kind);
    mob_type.finalize_spawn(spawning);
    finalize_mob(kind, spawning.mob, spawning.level);
    mob_type.after_finalize_spawn(spawning);
}

/// `Mob.finalizeSpawn`'s own part, after its type's: the follow range's bonus
/// (`random_spawn_bonus`, a triangle of ±0.11485 about 0 on the base, two `nextDouble`)
/// where it has none, and one hand in twenty the left (`nextFloat() < 0.05f`), drawn
/// from `level`.
pub(crate) fn finalize_mob(kind: u16, mob: &mut Mob, level: &mut dyn Draws) {
    let follow = attribute(ids::FOLLOW_RANGE);
    if let Some(instance) = mob.body.attributes.instance(kind, follow)
        && !instance.has_modifier(RANDOM_SPAWN_BONUS)
    {
        // `RandomSource.triangle(mode, deviation)`.
        let amount = 0.0 + 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
        instance.add_modifier(Modifier {
            id: RANDOM_SPAWN_BONUS.to_owned(),
            amount,
            operation: ADD_MULTIPLIED_BASE,
        });
    }
    mob.body.flags.left_handed = level.next_float() < 0.05;
}

/// `getMoonBrightness()` at the day time `day`: the dimension's brightness of the moon's
/// phase, `(day / 24000) % 8` (`DimensionType.MOON_BRIGHTNESS_PER_PHASE`).
pub(crate) fn moon_brightness(day: u64) -> f32 {
    const PER_PHASE: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];
    PER_PHASE[((day / 24_000) % 8) as usize]
}
