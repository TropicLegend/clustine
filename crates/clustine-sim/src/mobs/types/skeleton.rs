//! The skeleton (`Skeleton`; `docs/groundwork/n7k-brief.md`, sections 11.2 and 11.5): the
//! skeletons' body ([`super::skeletons`]), and its freezing. One that stands in powder
//! snow for 140 ticks shakes (`DATA_STRAY_CONVERSION_ID`, its synced flag) and, 301 ticks
//! on, becomes a stray holding its bow and wearing what it wore, with the level event
//! 1048 (`ConversionTracker`, counted before the rest of its tick, from where its last
//! tick left it). Leaving the snow at any moment starts the count again. Powder snow
//! does not hurt it (`canFreeze` false). Its tracker's counts are its record's, so that a
//! restore or a crossing in the middle goes on.

use clustine_data::{entity_types, sounds};

use super::skeletons::{self, change, state_of};
use super::{MobType, Spawning, template};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
use crate::shown::DataShown;
use clustine_world::BlockPos;

/// `DATA_STRAY_CONVERSION_ID`'s index.
const CONVERTING_INDEX: u8 = 16;

/// `ConversionTracker`'s two times for a skeleton: 140 ticks in the snow, then 300 more
/// (its conversion comes when the count goes below nothing).
const AFFLICTION_TIME: i32 = 140;
const CONVERSION_TIME: i32 = 300;

/// `LevelEvent.SOUND_SKELETON_TO_STRAY`.
const TO_STRAY: i32 = 1048;

/// The skeleton.
pub(crate) struct Skeleton;

/// `freezingTracker.tick()`, before the skeleton's own tick: nothing while it is dead or
/// without AI; afflicted (in powder snow as its last tick left it), its conversion counted
/// down and done below nothing, or its affliction counted up and the conversion started at
/// 140; out of the snow, both put back.
fn tick_tracker(tick: &mut MobTick<'_>) {
    if tick.mob.body.vitals.dying() || tick.mob.body.flags.no_ai {
        return;
    }
    let mut convert = false;
    change(&mut tick.mob, |own| {
        if own.in_powder_snow {
            match own.conversion {
                Some(left) => {
                    let left = left - 1;
                    own.conversion = Some(left);
                    convert = left < 0;
                }
                None => {
                    own.affliction += 1;
                    if own.affliction >= AFFLICTION_TIME {
                        own.conversion = Some(CONVERSION_TIME);
                    }
                }
            }
        } else {
            own.affliction = -1;
            own.conversion = None;
        }
    });
    if convert {
        // `doConversion`: `convertTo(STRAY, single(mob, true, true))`, no step after, the
        // level event at the stray's block. The skeleton's tick goes on, as the game's.
        tick.out.local.push(Local::Convert {
            into: entity_types::STRAY as u16,
            keep_equipment: true,
            sound: Some(TO_STRAY),
        });
    }
}

impl MobType for Skeleton {
    fn register_goals(&self, setup: &mut Setup) {
        skeletons::register_goals(setup);
    }

    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        skeletons::finalize_spawn(spawn);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        skeletons::walk_target_value(ai, pos)
    }

    fn avoids_sun(&self, mob: &Mob) -> bool {
        skeletons::avoids_sun(mob)
    }

    fn before_tick(&self, tick: &mut MobTick<'_>) {
        tick_tracker(tick);
    }

    /// What the tracker reads at the next tick: whether this one's move left it in
    /// powder snow.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        let in_snow = tick.living.in_powder_snow;
        change(&mut tick.mob, |own| own.in_powder_snow = in_snow);
    }

    fn can_freeze(&self) -> bool {
        false
    }

    /// `ConversionTracker.readAdditionalSaveData`: `FreezingTime` (0 where absent), and a
    /// `StrayConversionTime` other than −1 restarting the conversion with it.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let affliction = template::int(tag, "FreezingTime").unwrap_or(0);
        let conversion = template::int(tag, "StrayConversionTime").unwrap_or(-1);
        change(mob, |own| {
            own.affliction = affliction;
            own.conversion = (conversion != -1).then_some(conversion);
        });
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SKELETON_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SKELETON_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SKELETON_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SKELETON_STEP)
    }

    /// `isShaking()`, which the client draws from the tracker's flag.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let converting = state_of(mob).conversion.is_some();
        vec![(
            CONVERTING_INDEX,
            DataShown::Boolean(converting),
            !converting,
        )]
    }
}
