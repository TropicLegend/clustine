//! The pillager (`Pillager`; `n7k-brief.md` 3.7, `docs/groundwork/n9-plan.md` 8.4), part
//! K5 and N9's S7: an illager with a crossbow that floats, strolls, keeps away from a
//! creaking and looks; on patrol it holds its ground (`HoldGroundAttackGoal(10)`), and to
//! a target it charges its crossbow over 25 ticks of its use, showing it
//! (`IS_CHARGING_CROSSBOW`, data 17, which its arms are drawn by), and shoots it at 1.6
//! (`RangedCrossbowAttackGoal(1.0, 8)`, `crate::mobs::ai::ranged_weapons`). A raid's
//! pillager after wave 3 may carry the raid's crossbow of Quick Charge
//! (`illagers::apply_raid_buffs`).
//!
//! Named differences: its crossbow is never enchanted at easy (the one time in 300 that
//! would give it Piercing I on hard is drawn and lost); its inventory of five, which it
//! fills only once read from a save (`canPickUpLoot`) and with a white banner in a raid,
//! is not kept.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, ItemStack};
use serde::{Deserialize, Serialize};

use super::illagers;
use super::{MobType, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, FloatGoal, HoldGroundAttackGoal, LookAtPlayerGoal, RandomStrollGoal,
};
use crate::mobs::ai::ranged_weapons::{self, RangedCrossbowAttackGoal};
use crate::mobs::ai::sensing::{Class, Looked};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::making::enchant_spawned_equipment;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// The synced data's index of `IS_CHARGING_CROSSBOW`.
pub(crate) const CHARGING_INDEX: u8 = 17;

/// The creaking, which the illagers keep away from (none is made before its part).
pub(crate) const CREAKING: [u16; 1] = [entity_types::CREAKING as u16];

/// `AvoidEntityGoal(Creaking, 8, 1.0, 1.2)`.
pub(crate) const AVOID_CREAKING: AvoidEntityGoal = AvoidEntityGoal {
    class: Class::Kinds(&CREAKING),
    max_distance: 8.0,
    walk_speed: 1.0,
    sprint_speed: 1.2,
};

/// The pillager.
pub(crate) struct Pillager;

/// The pillager's own part of its raider state (`crate::mobs::types::illagers`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PillagerState {
    /// `IS_CHARGING_CROSSBOW`, synced at 17: set by its attack while it charges.
    pub(crate) charging: bool,
}

/// `setChargingCrossbow(charging)`.
pub(crate) fn set_charging(mob: &mut Mob, charging: bool) {
    illagers::update_own(mob, |own: &mut PillagerState| own.charging = charging);
}

/// `Pillager.performRangedAttack(target, power)`: `performCrossbowAttack(this, 1.6)`,
/// then `onCrossbowAttackPerformed`, its `noActionTime` back to nothing.
pub(crate) fn perform_ranged_attack(ai: &mut Ai<'_, '_>, target: &Looked, _power: f32) {
    ranged_weapons::perform_crossbow_attack(ai, target, 1.6);
    if let Some(since) = &mut ai.tick.mob.body.idle.no_action_since {
        // `noActionTime = 0` as the step leaves it: the next step counts it to 1.
        *since = ai.tick.lived + 1;
        ai.tick.mob.body.idle_gained.gained = 0;
    }
}

impl MobType for Pillager {
    /// `Pillager.registerGoals`, after `Raider`'s.
    fn register_goals(&self, setup: &mut Setup) {
        illagers::register_raider_goals(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, AVOID_CREAKING);
        setup.add_goal(
            2,
            HoldGroundAttackGoal {
                hostile_radius: 10.0,
            },
        );
        setup.add_goal(
            3,
            RangedCrossbowAttackGoal {
                speed: 1.0,
                radius: 8.0,
                perform: perform_ranged_attack,
                set_charging,
            },
        );
        setup.add_goal(8, RandomStrollGoal::new(0.6));
        setup.add_goal(
            9,
            LookAtPlayerGoal {
                probability: 1.0,
                ..LookAtPlayerGoal::new(Class::Player, 15.0)
            },
        );
        setup.add_goal(10, LookAtPlayerGoal::new(Class::Mob, 15.0));
        crate::mobs::ai::targets::register(setup, crate::mobs::ai::targets::PILLAGER);
    }

    /// `Pillager.finalizeSpawn`: the crossbow, its enchantment's draws, then
    /// `Raider`'s.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        // `populateDefaultEquipmentSlots`: the crossbow, with no draw.
        spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
            Some(ItemStack::new(items::CROSSBOW, 1));
        let special = spawn.difficulty.1;
        // `populateDefaultEquipmentEnchantments`, whose weapon's part is the pillager's
        // own `enchantSpawnedWeapon`: `Mob`'s draw, then one in 300 for Piercing with
        // a crossbow, then the armour's slots, which hold nothing.
        enchant_spawned_equipment(spawn.mob, slots::MAIN_HAND, spawn.level, 0.25, special);
        let _piercing = spawn.level.next_int(300) == 0
            && spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
                .as_ref()
                .is_some_and(|stack| stack.item == items::CROSSBOW);
        for slot in [slots::FEET, slots::LEGS, slots::CHEST, slots::HEAD] {
            enchant_spawned_equipment(spawn.mob, slot, spawn.level, 0.5, special);
        }
        illagers::finalize_spawn(entity_types::PILLAGER as u16, spawn, true);
    }

    /// `Pillager.getWalkTargetValue`: every place alike.
    fn walk_target_value(&self, _: &Ai<'_, '_>, _: BlockPos) -> f32 {
        0.0
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

    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        illagers::from_template(mob, tag);
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PILLAGER_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_PILLAGER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PILLAGER_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let charging = illagers::own_of::<PillagerState>(mob).charging;
        let mut values = illagers::shown(mob);
        values.push((CHARGING_INDEX, DataShown::Boolean(charging), !charging));
        values
    }
}
