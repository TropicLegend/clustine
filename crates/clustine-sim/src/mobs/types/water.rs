//! What the water creatures share (`docs/groundwork/n7-brief.md`, sections 6.5 and 6.7):
//! `WaterAnimal`'s and `AgeableWaterCreature`'s own air, the second move of a goal that
//! swims its mob itself, the sizes scaled by a variant, and taking a fish in a bucket
//! (`Bucketable`) and pouring one (`MobBucketItem`).
//!
//! Named differences: a bucket keeps nothing of its fish (Clustine's stacks carry no
//! components: the plan's question 4), so a fish poured from one is made as an egg makes
//! it, its variant and its size drawn again and its health full, its name and flags
//! lost; and the advancement a bucket's filling triggers is not kept (Clustine keeps no
//! advancements).

use clustine_data::{entity_types, items, sounds};
use clustine_world::ItemStack;

use super::{Did, Dimensions, Interaction, Used, animal};
use crate::living::Damage;
use crate::mobs::Mob;
use crate::mobs::ai::Ai;
use crate::mobs::tick::MobTick;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;

/// `WaterAnimal.handleAirSupply(level, air)` and `AgeableWaterCreature.handleAirSupply
/// (air)`, after the living body's air rule, which it overwrites: out of water, from the
/// air it had as the tick began, one less a tick and at -20 a drowning hit of 2 with the
/// air back to nothing (and no bubbles: the living rule's entity event 67 is not sent);
/// in water, or dead, all 300 at once.
pub(crate) fn handle_air_supply(tick: &mut MobTick<'_>, air: i32) {
    let alive = !tick.mob.body.vitals.dying();
    if alive && !tick.living.body.in_water {
        tick.living.air = air - 1;
        // `shouldTakeDrowningDamage`: at -20 or below.
        if tick.living.air <= -20 {
            tick.living.air = 0;
            tick.living.hurt(Damage::Drown, 2.0);
        }
    } else {
        tick.living.air = 300;
    }
}

/// `moveRelative(speed, (xxa, yya, zza))` and `move(SELF, motion)` of a goal that moves
/// its mob a second time in its tick (the brief's W2): its inputs as they are when the
/// goal ticks, turned by its yaw.
pub(crate) fn move_again(ai: &mut Ai<'_, '_>, speed: f32) {
    let inputs = &ai.tick.mob.body.inputs;
    let input = [
        f64::from(inputs.xxa),
        f64::from(inputs.yya),
        f64::from(inputs.zza),
    ];
    let yaw = ai.tick.yaw;
    let world = ai.tick.world;
    ai.tick.living.move_again(world, speed, input, yaw);
}

/// `EntityDimensions.scale(factor)` of a type's size: each measure times the factor, in
/// floats.
pub(crate) fn scaled(width: f32, height: f32, eye_height: f32, factor: f32) -> Dimensions {
    Dimensions {
        width: width * factor,
        height: height * factor,
        eye_height: eye_height * factor,
    }
}

/// The bucket a mob of the type `kind` is taken in (`getBucketItemStack`).
pub(crate) fn bucket_of(kind: u16) -> Option<i32> {
    Some(match i32::from(kind) {
        entity_types::COD => items::COD_BUCKET,
        entity_types::SALMON => items::SALMON_BUCKET,
        entity_types::PUFFERFISH => items::PUFFERFISH_BUCKET,
        entity_types::TROPICAL_FISH => items::TROPICAL_FISH_BUCKET,
        entity_types::AXOLOTL => items::AXOLOTL_BUCKET,
        entity_types::TADPOLE => items::TADPOLE_BUCKET,
        _ => return None,
    })
}

/// The type of mob the bucket `item` pours (`MobBucketItem.type`).
pub(crate) fn poured_by(item: i32) -> Option<u16> {
    let kind = match item {
        items::COD_BUCKET => entity_types::COD,
        items::SALMON_BUCKET => entity_types::SALMON,
        items::PUFFERFISH_BUCKET => entity_types::PUFFERFISH,
        items::TROPICAL_FISH_BUCKET => entity_types::TROPICAL_FISH,
        items::AXOLOTL_BUCKET => entity_types::AXOLOTL,
        items::TADPOLE_BUCKET => entity_types::TADPOLE,
        _ => return None,
    };
    u16::try_from(kind).ok()
}

/// `Bucketable.bucketMobPickup(player, hand, mob)`: a water bucket used on a living fish
/// takes it, with the bucket's filling sound at the fish (heard by the player too), its
/// bucket given to the player (`ItemUtils.createFilledResult(stack, player, bucket,
/// false)`: a creative player keeps the water bucket and is given a fish bucket
/// whether or not one is held already), and the fish gone; nothing otherwise.
pub(crate) fn bucket_pickup(used: &mut Used<'_>) -> Option<Interaction> {
    bucket_pickup_sounding(used, sounds::ITEM_BUCKET_FILL_FISH)
}

/// [`bucket_pickup`] with the type's own filling sound (`getPickupSound()`: an
/// axolotl's, a tadpole's).
pub(crate) fn bucket_pickup_sounding(used: &mut Used<'_>, sound: i32) -> Option<Interaction> {
    if !animal::is_item(used.held, items::WATER_BUCKET) || used.mob.body.vitals.dying() {
        return None;
    }
    let bucket = bucket_of(used.kind)?;
    used.did.push(Did::Sound {
        sound,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    used.did.push(Did::Give {
        stack: ItemStack {
            item: bucket,
            count: 1,
        },
        no_duplicates: false,
    });
    used.did.push(Did::Discard);
    Some(Interaction::Success)
}

/// Whether the mob's eyes are in water (`isEyeInFluid(#water)`), as the living body
/// finds it now.
pub(crate) fn eye_in_water(tick: &MobTick<'_>) -> bool {
    tick.living.eye_in_water(tick.world)
}

/// `getVoicePitch()` of a sound the type makes of itself in its tick (`makeSound`): two
/// draws of its own, for the sound `sound` at its volume `volume`.
pub(crate) fn make_sound(tick: &mut MobTick<'_>, sound: i32, volume: f32, baby: bool) {
    let pitch = crate::mobs::body::hurt::voice_pitch(tick.draws, baby);
    tick.out.events.push(crate::mobs::body::Event::Sound {
        sound,
        volume,
        pitch,
    });
}

/// Whether the mob is alive (`isAlive`): not dying.
pub(crate) fn alive(mob: &Mob) -> bool {
    !mob.body.vitals.dying()
}
