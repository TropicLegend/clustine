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
//! Its weapon goal (`reassessWeaponGoal`, n7k AbstractSkeleton 269–327) is at priority 4
//! after the others: `RangedBowAttackGoal` (`crate::mobs::ai::ranged_weapons`) while its
//! bow's hand holds a bow, at the type's interval (the skeleton's and the stray's 20 on
//! hard and 40 else, the bogged's and the parched's 50 and 70), else the melee goal at
//! 1.2. The goals are registered at every AI step from what the mob holds, so the goal
//! is reassessed at its spawn, its load and any change of its hands alike. Its shot
//! (`performRangedAttack`, [`perform_ranged_attack`]) is an arrow of the type's: the
//! stray's slows for 600 ticks, the bogged's poisons for 100, the parched's weakens for
//! 600 (`getArrow`'s `Arrow.addEffect`, on the arrow's pickup stack).
//!
//! Named differences: the body of a skeleton riding a spider does not take the spider's
//! turn (`rideTick`); Halloween's pumpkins are never drawn for (`n7k-plan.md`, section
//! 10); a change of weapon while its goal runs puts the other goal in the slot without
//! the running one's `stop` (`removeGoal`'s), which no mob meets in N9, none changing
//! what it holds after its spawn.

use clustine_data::{entity_types, items};
use clustine_world::{BlockPos, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::Spawning;
use super::monster_ai;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, LookAtPlayerGoal, MeleeAttackGoal, RandomLookAroundGoal, RandomStrollGoal,
};
use crate::mobs::ai::navigation;
use crate::mobs::ai::ranged_weapons::{self, RangedBowAttackGoal};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Looked};
use crate::mobs::ai::targets;
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

/// `getHardAttackInterval()` and `getAttackInterval()` of the skeleton type `kind`: the
/// bogged's and the parched's longer (`INCREASED_*`).
fn attack_intervals(kind: u16) -> (i32, i32) {
    let increased = [entity_types::BOGGED, entity_types::PARCHED].contains(&i32::from(kind));
    if increased { (50, 70) } else { (20, 40) }
}

/// The effect a skeleton type's arrows carry (`getArrow`'s `Arrow.addEffect`): the
/// stray's slowness for 600 ticks, the bogged's poison for 100, the parched's weakness
/// for 600; none of the skeleton's.
pub(crate) fn arrow_effect(kind: u16) -> Option<(&'static str, i32)> {
    match i32::from(kind) {
        entity_types::STRAY => Some(("minecraft:slowness", 600)),
        entity_types::BOGGED => Some(("minecraft:poison", 100)),
        entity_types::PARCHED => Some(("minecraft:weakness", 600)),
        _ => None,
    }
}

/// The pickup stack of an arrow of the entity type `arrow` shot by a skeleton of the
/// type `shooter`, given the type's effect where it is an `Arrow` (not a spectral one).
fn with_effect(shooter: u16, arrow: u16, stack: ItemStack) -> ItemStack {
    match arrow_effect(shooter) {
        Some((effect, duration)) if i32::from(arrow) == entity_types::ARROW => {
            ranged_weapons::with_effect_added(&stack, effect, duration)
        }
        _ => stack,
    }
}

/// `AbstractSkeleton.performRangedAttack(target, power)` with the type's `getArrow`
/// (`ranged_weapons::skeleton_shot`).
pub(crate) fn perform_ranged_attack(ai: &mut Ai<'_, '_>, target: &Looked, power: f32) {
    fn plain(_: u16, stack: ItemStack) -> ItemStack {
        stack
    }
    fn stray(kind: u16, stack: ItemStack) -> ItemStack {
        with_effect(entity_types::STRAY as u16, kind, stack)
    }
    fn bogged(kind: u16, stack: ItemStack) -> ItemStack {
        with_effect(entity_types::BOGGED as u16, kind, stack)
    }
    fn parched(kind: u16, stack: ItemStack) -> ItemStack {
        with_effect(entity_types::PARCHED as u16, kind, stack)
    }
    let effect: ranged_weapons::ArrowEffect = match i32::from(ai.tick.kind) {
        entity_types::STRAY => stray,
        entity_types::BOGGED => bogged,
        entity_types::PARCHED => parched,
        _ => plain,
    };
    ranged_weapons::skeleton_shot(ai, target, power, effect);
}

/// `reassessWeaponGoal()` of a skeleton of the type `kind` holding what `mob` holds: the
/// bow goal at the difficulty's interval where the bow's hand holds a bow, else the melee
/// goal (`AbstractSkeleton$1`: `MeleeAttackGoal(1.2, false)`, aggressive while it runs,
/// as Clustine's is), at 4.
fn reassess_weapon_goal(kind: u16, mob: &Mob, setup: &mut Setup) {
    let hand = ranged_weapons::weapon_hand(mob, items::BOW);
    let bow = ranged_weapons::in_hand(mob, hand).is_some_and(|stack| stack.item == items::BOW);
    if bow {
        let (hard, other) = attack_intervals(kind);
        let hard_world = clustine_world::Difficulty::WORLD == clustine_world::Difficulty::Hard;
        setup.add_goal(
            4,
            RangedBowAttackGoal {
                speed: 1.0,
                attack_interval_min: if hard_world { hard } else { other },
                radius: 15.0,
                perform: perform_ranged_attack,
            },
        );
    } else {
        setup.add_goal(
            4,
            MeleeAttackGoal {
                speed: 1.2,
                following_even_if_not_seen: false,
            },
        );
    }
}

/// `AbstractSkeleton.registerGoals` of a skeleton of the type `kind`, then the weapon
/// goal `reassessWeaponGoal` adds after them for what `mob` holds (none: a fresh
/// skeleton's, which holds nothing until its spawn gives it its bow).
pub(crate) fn register_goals(kind: u16, mob: Option<&Mob>, setup: &mut Setup) {
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
    // `reassessWeaponGoal`'s, removed and added again after the others.
    match mob {
        Some(mob) => reassess_weapon_goal(kind, mob, setup),
        None => reassess_weapon_goal(kind, &Mob::fresh(), setup),
    }
    targets::register(setup, targets::SKELETON);
}

/// `AbstractSkeleton.finalizeSpawn` after `Monster`'s (the tail): the armour's draws and
/// the bow in the main hand (`populateDefaultEquipmentSlots`), the enchantments' draws
/// (the bow's, and each armour slot's that holds something), the weapon goal reassessed
/// (at its next AI step, from the bow it now holds), and the loot pick-up's draw against
/// `0.55 × special`. On Halloween the game draws for a pumpkin; Clustine never is on it.
pub(crate) fn finalize_spawn(spawn: &mut Spawning<'_>) {
    let special = spawn.difficulty.1;
    let hard = clustine_world::Difficulty::WORLD == clustine_world::Difficulty::Hard;
    making::populate_default_equipment_slots(spawn.mob, spawn.level, special, hard);
    spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
        Some(ItemStack::new(items::BOW, 1));
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
