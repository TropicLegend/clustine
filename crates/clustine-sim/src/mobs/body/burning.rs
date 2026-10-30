//! What a monster's body does of itself in `Monster.aiStep` and `Mob.aiStep`
//! (`docs/groundwork/n7k-brief.md`, sections 3.1 and 3.3): its idle time counted twice more
//! in the light, and the undead's burning in daylight.

use clustine_blocks::data::entity_in_tag;
use clustine_data::{ItemComponents, attributes, entity_types};
use clustine_world::BlockPos;

use super::ladder::{self, Family};
use crate::living::slots;
use crate::mobs::tick::MobTick;

/// `LevelReader.getLightLevelDependentMagicValue(pos)` at the mob's eyes, or 0 where its
/// view has no light (`Entity`'s: 0 where the chunk is not loaded). The raw brightness is
/// the sky's light less its darkening or the block's, the larger
/// (`LevelLightEngine.getRawBrightness`), and the level's ambient light is lerped in.
pub(crate) fn magic_value(tick: &MobTick<'_>) -> f32 {
    let Some(light) = tick.world.light(eye_block(tick)) else {
        return 0.0;
    };
    let raw = (i32::from(light.sky) - tick.world.sky_darken()).max(i32::from(light.block));
    let raw = u8::try_from(raw.clamp(0, 15)).expect("a light level");
    crate::generation::spawning::magic_value(tick.world.dimension(), raw)
}

/// `BlockPos.containing(getX(), getEyeY(), getZ())`.
fn eye_block(tick: &MobTick<'_>) -> BlockPos {
    let at = tick.living.body.position;
    crate::physics::containing([at[0], at[1] + f64::from(tick.living.eye_height), at[2]])
}

/// `Monster.updateNoActionTime()`, at the head of `Monster.aiStep`: two more in light
/// whose magic value is above a half, and two more always for a raider (`Raider`'s own),
/// where the idle time counts at all.
pub(crate) fn monster_idles(tick: &mut MobTick<'_>) {
    if ladder::of(tick.kind) != Family::Monster || tick.mob.body.idle.no_action_since.is_none() {
        return;
    }
    let raider = entity_in_tag(i32::from(tick.kind), "minecraft:raiders");
    if raider || magic_value(tick) > 0.5 {
        tick.mob.body.idle_gained.gained = tick.mob.body.idle_gained.gained.saturating_add(2);
    }
}

/// Whether the day timeline's `gameplay/monsters_burn` holds at the overworld clock's
/// `day` (`timeline/day.json`: false from tick 12542 of the day, true from 23460).
pub(crate) fn monsters_burn(day: u64) -> bool {
    !(12_542..23_460).contains(&(day % crate::time::DAY))
}

/// `Mob.sunProtectionSlot()`: the head, but the body for the zombie horse and the zombie
/// nautilus (`ZombieHorse.txt` 314–317, `ZombieNautilus.txt` 28–31).
fn sun_protection_slot(kind: u16) -> u8 {
    match i32::from(kind) {
        entity_types::ZOMBIE_HORSE | entity_types::ZOMBIE_NAUTILUS => slots::BODY,
        _ => slots::HEAD,
    }
}

/// `Mob.burnUndead()` for a type in `#burn_in_daylight`, after `LivingEntity.aiStep`:
/// in the day's burning hours, in light above a half, at the mob's own draw against
/// `(b - 0.4) * 2 / 30`, dry, and under the open sky, it catches fire for eight seconds,
/// or its helmet (the body's armour for an undead horse) keeps the sun off, a wearing one
/// drawing its wear. **Named**: the wear is drawn and not kept, as item stacks have no
/// damage yet (`n7k-plan.md`, section 10).
pub(crate) fn burn_undead(tick: &mut MobTick<'_>) {
    if tick.mob.body.vitals.dying()
        || !entity_in_tag(i32::from(tick.kind), "minecraft:burn_in_daylight")
        || !monsters_burn(tick.world.day())
    {
        return;
    }
    let b = magic_value(tick);
    // `isSunBurnTick`: the draw is made whenever the light passes, before the wet and sky
    // tests.
    if !(b > 0.5 && tick.draws.next_float() * 30.0 < (b - 0.4) * 2.0) {
        return;
    }
    let wet =
        tick.living.body.in_water || tick.living.in_powder_snow || tick.living.was_in_powder_snow;
    let sky = tick
        .world
        .light(eye_block(tick))
        .is_some_and(|light| light.sky >= 15);
    if wet || !sky {
        return;
    }
    let slot = usize::from(sun_protection_slot(tick.kind));
    if let Some(stack) = &tick.mob.body.equipment.slots[slot] {
        if ItemComponents::of(stack.item).max_damage.is_some() {
            let _wear = tick.draws.next_int(2);
        }
        return;
    }
    // `igniteForSeconds(8)`: `Mth.floor(8 * 20)` ticks, scaled by `burning_time` and
    // rounded up (`LivingEntity.igniteForTicks`).
    let scale = tick
        .mob
        .body
        .attributes
        .value(tick.kind, super::attribute(attributes::BURNING_TIME));
    let ticks = (160.0 * scale).ceil() as i32;
    tick.living.ignite_for_ticks(ticks);
}
