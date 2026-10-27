//! The bogged (`Bogged`; `n7k-brief.md` 11.2): a skeleton ([`super::skeletons`]) of 16
//! health and its own sounds, that shears: shears on one not yet sheared play
//! `bogged_shear` from the players' source, drop its table's two mushrooms
//! (`shearing/bogged`) at the top of its box, shear it (its synced flag) and post the
//! `shear` game event. Named: its arrows' poison is N9's; the shears wear nothing, every
//! player being creative.

use clustine_data::{items, sounds};
use clustine_world::{BlockPos, ItemStack};

use super::skeletons::{self, change, state_of};
use super::{Did, Interaction, MobType, Spawning, Used, animal, template};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::loot;
use crate::shown::DataShown;

/// `DATA_SHEARED`'s index.
const SHEARED_INDEX: u8 = 16;

/// The bogged.
pub(crate) struct Bogged;

/// `Bogged.shear(level, PLAYERS, shears)`: the sound at the bogged, heard by all; its
/// table's items, each at the top of its box (`spawnAtLocation(stack, getBbHeight())`, no
/// push of its own); then sheared.
fn shear(used: &mut Used<'_>) {
    used.did.push(Did::Sound {
        sound: sounds::ENTITY_BOGGED_SHEAR,
        source: animal::SOUND_SOURCE_PLAYERS,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    let drops = loot::named_drops(
        "minecraft:shearing/bogged",
        used.kind,
        false,
        loot::Facts::default(),
        used.draws,
    );
    // `getBbHeight()`.
    let height = clustine_data::EntityTypeInfo::of(i32::from(used.kind)).height;
    let [x, y, z] = used.position;
    for stack in drops {
        used.did.push(Did::Drop {
            stack,
            at: [x, y + f64::from(height), z],
            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
            push: None,
        });
    }
    change(used.mob, |own| own.sheared = true);
}

impl MobType for Bogged {
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

    /// `Bogged.mobInteract`: shears on one ready for shearing (not sheared) shear it;
    /// anything else is a monster's use, which does nothing.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let shears = used
            .held
            .as_ref()
            .is_some_and(|stack: &ItemStack| stack.count > 0 && stack.item == items::SHEARS);
        if shears && !state_of(used.mob).sheared {
            shear(used);
            used.did.push(Did::GameEvent("minecraft:shear"));
            return Interaction::Success;
        }
        Interaction::Pass
    }

    /// `readAdditionalSaveData`: `sheared`, false where absent.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let sheared = template::boolean(tag, "sheared").unwrap_or(false);
        change(mob, |own| own.sheared = sheared);
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_BOGGED_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_BOGGED_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_BOGGED_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_BOGGED_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let sheared = state_of(mob).sheared;
        vec![(SHEARED_INDEX, DataShown::Boolean(sheared), !sheared)]
    }
}
