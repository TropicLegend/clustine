//! The vindicator (`Vindicator`; `n7k-brief.md` 3.7), part K5: an illager with an iron
//! axe that floats, strolls, keeps away from a creaking and looks.
//!
//! Named differences: its axe is never enchanted, as item stacks carry no enchantments
//! (the draw is made); a vindicator named Johnny is kept as one (`Johnny`), which only
//! its Johnny target goal reads, and no mob is named in Clustine but by a template's `Johnny`;
//! the doors it opens and breaks are a raid's (N10), and its navigation, which the
//! game lets open doors from its `finalizeSpawn` to its first AI step, opens none.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, ItemStack};
use serde::{Deserialize, Serialize};

use super::illagers;
use super::pillager::AVOID_CREAKING;
use super::{MobType, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{FloatGoal, LookAtPlayerGoal, RandomStrollGoal};
use crate::mobs::ai::selector::flags;
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::making::populate_default_equipment_enchantments;
use crate::mobs::types::tamable::Idle;
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
    /// `Vindicator.registerGoals`, after `Raider`'s.
    fn register_goals(&self, setup: &mut Setup) {
        illagers::register_raider_goals(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, AVOID_CREAKING);
        // `VindicatorBreakDoorGoal` and `RaiderOpenDoorGoal`, a raid's;
        // `HoldGroundAttackGoal(10)` and `MeleeAttackGoal(1.0, false)`, a target's.
        setup.add_goal(2, Idle { flags: flags::MOVE });
        setup.add_goal(3, Idle { flags: 0 });
        setup.add_goal(4, Idle { flags: flags::MOVE });
        setup.add_goal(
            5,
            Idle {
                flags: flags::MOVE | flags::LOOK,
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
    /// raid (no raid before N10), with no draw, and its enchantment's draws.
    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
            Some(ItemStack::new(items::IRON_AXE, 1));
        populate_default_equipment_enchantments(spawn.mob, spawn.level, spawn.difficulty.1);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        -super::animal::light_cost(ai, pos)
    }

    fn remove_when_far_away(&self, _: u16, mob: &Mob, _: u32, distance: f64) -> bool {
        illagers::remove_when_far_away(mob, distance)
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
