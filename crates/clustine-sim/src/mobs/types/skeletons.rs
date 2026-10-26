//! What the skeleton, the stray, the bogged and the parched share (`AbstractSkeleton`;
//! `docs/groundwork/n7k-brief.md`, section 11.2): no float (a skeleton sinks and walks
//! the bottom); by day, bareheaded, its paths kept out of the sun (`RestrictSunGoal`) and,
//! burning, a walk into shade (`FleeSunGoal`); a run from any wolf within six blocks; a
//! stroll and its looks. Its bow is given at its spawn with the game's draws.
//!
//! What every one of them keeps is one state ([`SkeletonState`]): the navigation's
//! `avoidSun`, which `RestrictSunGoal` sets and the path's trim reads (the game keeps it
//! in the navigation); the skeleton's freezing tracker; the bogged's shearing.
//!
//! Named differences: the bow and melee goals and the target goals are N9's (they hold
//! their slots and draw what the game's draw without a target, `monster_ai.rs`), so
//! `reassessWeaponGoal` changes nothing a player sees; the body of a skeleton riding a
//! spider does not take the spider's turn (`rideTick`); Halloween's pumpkins are never
//! drawn for (`n7k-plan.md`, section 10).

use clustine_data::{entity_types, items};
use clustine_world::{BlockPos, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::Spawning;
use super::monster_ai::{self, AttackOfN9, TargetOfN9};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, LookAtPlayerGoal, RandomLookAroundGoal, RandomStrollGoal,
};
use crate::mobs::ai::navigation;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::making;
use crate::physics::Vec3;

/// The version of a skeleton's type state.
const VERSION: u8 = 1;

/// What a skeleton of any kind keeps beside its body and mind.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SkeletonState {
    /// `GroundPathNavigation.avoidSun`, as `RestrictSunGoal` sets it.
    pub(crate) avoid_sun: bool,
    /// `isInPowderSnow` as its last tick left it, which the skeleton's freezing tracker
    /// reads before the next (the game keeps it in the field, unsaved).
    pub(crate) in_powder_snow: bool,
    /// `ConversionTracker.afflictionTime` (`FreezingTime`): the ticks it has stood in
    /// powder snow, −1 once it has left it.
    pub(crate) affliction: i32,
    /// `conversionTime` (`StrayConversionTime`) while it converts (the synced flag
    /// `DATA_STRAY_CONVERSION_ID`): the ticks left before it is a stray.
    pub(crate) conversion: Option<i32>,
    /// The bogged's `DATA_SHEARED` (`sheared`).
    pub(crate) sheared: bool,
}

/// A skeleton's state from its type state; a record of nothing, or one this build cannot
/// read, as a fresh skeleton's.
pub(crate) fn state_of(mob: &Mob) -> SkeletonState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return SkeletonState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// Its state written back; a fresh one's as nothing.
pub(crate) fn put_state(mob: &mut Mob, own: &SkeletonState) {
    mob.state = if *own == SkeletonState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

/// Its state changed by `change`, written only where it changed.
pub(crate) fn change(mob: &mut Mob, change: impl FnOnce(&mut SkeletonState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put_state(mob, &own);
    }
}

/// The wolf, which every skeleton runs from.
const WOLVES: &[u16] = &[entity_types::WOLF as u16];

/// `AbstractSkeleton.registerGoals`, then the weapon goal `reassessWeaponGoal` adds after
/// them (`bowGoal` or `meleeGoal` at 4: N9's).
pub(crate) fn register_goals(setup: &mut Setup) {
    setup.add_goal(2, RestrictSunGoal);
    setup.add_goal(3, FleeSunGoal { speed: 1.0 });
    setup.add_goal(
        3,
        AvoidEntityGoal {
            class: Class::Kinds(WOLVES),
            max_distance: 6.0,
            walk_speed: 1.0,
            sprint_speed: 1.2,
        },
    );
    setup.add_goal(5, RandomStrollGoal::avoiding_water(1.0));
    setup.add_goal(6, LookAtPlayerGoal::new(Class::Player, 8.0));
    setup.add_goal(6, RandomLookAroundGoal);
    // `reassessWeaponGoal`'s, removed and added again after the others: the bow's
    // (`RangedBowAttackGoal`) or the melee's, both of moving and looking.
    setup.add_goal(
        4,
        AttackOfN9 {
            flags: flags::MOVE | flags::LOOK,
        },
    );
    // Who last hurt it, players, iron golems, and baby turtles on land (every 10).
    setup.add_target(1, TargetOfN9::HURT_BY);
    setup.add_target(2, TargetOfN9::NEAREST);
    setup.add_target(3, TargetOfN9::NEAREST);
    setup.add_target(3, TargetOfN9::NEAREST);
}

/// `AbstractSkeleton.finalizeSpawn` after `Monster`'s (the tail): the armour's draws and
/// the bow in the main hand (`populateDefaultEquipmentSlots`), the enchantments' draws
/// (the bow's, and each armour slot's that holds something), the weapon goal reassessed
/// (nothing to do until N9), and the loot pick-up's draw against `0.55 × special`. On
/// Halloween the game draws for a pumpkin; Clustine never is on it.
pub(crate) fn finalize_spawn(spawn: &mut Spawning<'_>) {
    let special = spawn.difficulty.1;
    let hard = clustine_world::Difficulty::WORLD == clustine_world::Difficulty::Hard;
    making::populate_default_equipment_slots(spawn.mob, spawn.level, special, hard);
    spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = Some(ItemStack {
        item: items::BOW,
        count: 1,
    });
    making::populate_default_equipment_enchantments(spawn.mob, spawn.level, special);
    spawn.mob.body.flags.picks_up_loot = spawn.level.next_float() < 0.55 * special;
}

/// `RestrictSunGoal(mob)`: while it is bright outside and its head is bare, its paths are
/// cut before the sun (`setAvoidSun`). No flags.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RestrictSunGoal;

impl Goal for RestrictSunGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    /// `isBrightOutside()`, nothing on its head, and a ground navigation (every
    /// skeleton's).
    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        monster_ai::bright_outside(ai)
            && ai.tick.mob.body.equipment.slots[usize::from(slots::HEAD)].is_none()
            && ai.config.navigation.kind == navigation::Kind::Ground
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        change(&mut ai.tick.mob, |own| own.avoid_sun = true);
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        change(&mut ai.tick.mob, |own| own.avoid_sun = false);
    }
}

/// `FleeSunGoal`'s state: where it hides (`wantedX`, `wantedY`, `wantedZ`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct FleeingSun {
    pub(crate) wanted: Vec3,
}

/// `FleeSunGoal(mob, speed)`: burning by day under the open sky, bareheaded and with no
/// target, off to a shaded place it does not like the light of.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FleeSunGoal {
    pub(crate) speed: f64,
}

impl FleeSunGoal {
    /// `getHidePos()`: of ten tries of its own random about its block, the first the sky
    /// does not see and whose walk target value is below nothing (a monster's: light
    /// worth more than a half), at its bottom's centre.
    fn hide_pos(ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        let at = ai.block_position();
        for _ in 0..10 {
            let dx = ai.tick.draws.next_int(20) as i32 - 10;
            let dy = ai.tick.draws.next_int(6) as i32 - 3;
            let dz = ai.tick.draws.next_int(20) as i32 - 10;
            let pos = BlockPos::new(at.x + dx, at.y + dy, at.z + dz);
            if !navigation::can_see_sky(ai, pos)
                && ai.tick.mob_type.walk_target_value(ai, pos) < 0.0
            {
                return Some([
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y),
                    f64::from(pos.z) + 0.5,
                ]);
            }
        }
        None
    }
}

impl Goal for FleeSunGoal {
    type State = FleeingSun;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut FleeingSun, ai: &mut Ai<'_, '_>) -> bool {
        if sensing::target(ai).is_some() || !monster_ai::bright_outside(ai) {
            return false;
        }
        // `isOnFire()`.
        if ai.tick.living.fire_ticks <= 0 {
            return false;
        }
        let at = ai.block_position();
        if !navigation::can_see_sky(ai, at)
            || ai.tick.mob.body.equipment.slots[usize::from(slots::HEAD)].is_some()
        {
            return false;
        }
        match Self::hide_pos(ai) {
            Some(wanted) => {
                state.wanted = wanted;
                true
            }
            None => false,
        }
    }

    fn can_continue(&self, _: &mut FleeingSun, ai: &mut Ai<'_, '_>) -> bool {
        !navigation::is_done(ai)
    }

    fn start(&self, state: &mut FleeingSun, ai: &mut Ai<'_, '_>) {
        navigation::move_to(ai, state.wanted, self.speed);
    }
}

/// `MobType::walk_target_value` of a skeleton: a monster's.
pub(crate) fn walk_target_value(ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
    monster_ai::walk_target_value(ai, pos)
}

/// `MobType::avoids_sun` of a skeleton: as its `RestrictSunGoal` set it.
pub(crate) fn avoids_sun(mob: &Mob) -> bool {
    state_of(mob).avoid_sun
}
