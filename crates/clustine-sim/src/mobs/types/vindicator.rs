//! The vindicator (`Vindicator`; `n7k-brief.md` 3.7), part K5: an illager with an iron
//! axe that floats, strolls, keeps away from a creaking and looks. With a target it
//! chases and hits it (`MeleeAttackGoal(1.0, false)`), and a patrolling one first holds
//! its ground and calls the raiders about (`HoldGroundAttackGoal(10)`;
//! `docs/groundwork/n9-plan.md` section 8.4).
//!
//! In a raid it comes with its axe from the raid (`applyRaidBuffs`, enchanted by the
//! raid's odds), opens the wooden doors in its way (`RaiderOpenDoorGoal`) and would break
//! doors on normal and hard (`crate::mobs::ai::raider_goals`).
//!
//! Named differences: its axe out of a raid is never enchanted, as its spawn's
//! enchantment is not built (the draw is made); a vindicator named Johnny is kept as one
//! (`Johnny`), which only its Johnny target goal reads, and no mob is named in Clustine
//! but by a template's `Johnny`; its navigation opens doors while it is of a raid
//! (`setCanOpenDoors(isRaided)`, where the game asks whether a raid is near it), and none
//! from its `finalizeSpawn` to its first AI step, where the game lets it.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, ItemStack};
use serde::{Deserialize, Serialize};

use super::illagers;
use super::pillager::AVOID_CREAKING;
use super::{MobType, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, HoldGroundAttackGoal, LookAtPlayerGoal, MeleeAttackGoal, RandomStrollGoal,
};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup, raider_goals};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::making::populate_default_equipment_enchantments;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// The vindicator.
pub(crate) struct Vindicator;

/// The vindicator's own part of its raider state (`crate::mobs::types::illagers`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct VindicatorState {
    /// `isJohnny` (`Johnny`).
    pub(crate) johnny: bool,
}

impl MobType for Vindicator {
    /// `Vindicator.registerGoals`, after `Raider`'s, and its navigation's doors
    /// (`customServerAiStep`'s `setCanOpenDoors(isRaided)`), open while it is of a raid.
    fn register_goals_of(&self, mob: &Mob, setup: &mut Setup) {
        self.register_goals(setup);
        setup.config.navigation.flags.can_open_doors = illagers::state_of(mob).raid.is_some();
    }

    /// `Vindicator.registerGoals`, after `Raider`'s.
    fn register_goals(&self, setup: &mut Setup) {
        illagers::register_raider_goals(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, AVOID_CREAKING);
        setup.add_goal(2, raider_goals::VindicatorBreakDoorGoal);
        setup.add_goal(3, raider_goals::RaiderOpenDoorGoal);
        setup.add_goal(
            4,
            HoldGroundAttackGoal {
                hostile_radius: 10.0,
            },
        );
        setup.add_goal(
            5,
            MeleeAttackGoal {
                speed: 1.0,
                following_even_if_not_seen: false,
            },
        );
        setup.add_goal(8, RandomStrollGoal::new(0.6));
        setup.add_goal(
            9,
            LookAtPlayerGoal {
                probability: 1.0,
                ..LookAtPlayerGoal::new(Class::Player, 3.0)
            },
        );
        setup.add_goal(10, LookAtPlayerGoal::new(Class::Mob, 8.0));
        crate::mobs::ai::targets::register(setup, crate::mobs::ai::targets::VINDICATOR);
    }

    /// `Raider.finalizeSpawn`, which the vindicator's calls first.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        illagers::finalize_spawn(entity_types::VINDICATOR as u16, spawn, true);
    }

    /// The rest of `Vindicator.finalizeSpawn`, after `Mob`'s: an iron axe outside a
    /// raid (`populateDefaultEquipmentSlots`), with no draw, and its enchantment's draws,
    /// none for an empty hand.
    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        if illagers::state_of(spawn.mob).raid.is_none() {
            spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
                Some(ItemStack::new(items::IRON_AXE, 1));
        }
        populate_default_equipment_enchantments(spawn.mob, spawn.level, spawn.difficulty.1);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        -super::animal::light_cost(ai, pos)
    }

    fn remove_when_far_away(&self, _: u16, mob: &Mob, _: u32, distance: f64) -> bool {
        illagers::remove_when_far_away(mob, distance)
    }

    /// `Raider.aiStep`'s own part (`crate::mobs::ai::raider_goals`).
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        illagers::ai_step(tick);
    }

    /// `Raider.hurtServer`'s word to its raid, before the hurt lands.
    fn before_hurt_server(&self, struck: &mut Struck<'_>, _: &DamageSource, _: f32) {
        illagers::hurt(struck);
    }

    /// `Raider.die`'s word to its raid.
    fn on_die(&self, struck: &mut Struck<'_>, source: &DamageSource) {
        illagers::died(struck, source);
    }

    fn requires_custom_persistence(&self, mob: &Mob) -> bool {
        illagers::requires_custom_persistence(mob)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        illagers::experience(5, mob, draws)
    }

    /// `readAdditionalSaveData`: the raider's, and `Johnny`.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        illagers::from_template(mob, tag);
        illagers::update_own(mob, |own: &mut VindicatorState| {
            own.johnny = super::template::boolean(tag, "Johnny").unwrap_or(false);
        });
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_VINDICATOR_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_VINDICATOR_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_VINDICATOR_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        illagers::shown(mob)
    }
}
