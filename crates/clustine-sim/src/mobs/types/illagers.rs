//! What the witch and the illagers share (`n7k-brief.md` 3.1, 3.6 and 3.7): the layers
//! `PatrollingMonster`, `Raider`, `AbstractIllager` and `SpellcasterIllager` add to a
//! monster, and the one type state the four types keep, each its own part of it.
//!
//! Named differences: no raid exists before N10, so the raid's goals hold their places
//! as goals that never start, a raider never joins a raid nor celebrates, and its
//! `Wave` and `CanJoinRaid` are kept and never read; no patrol is made before N9, so
//! `LongDistancePatrolGoal` holds its place too and a patrol's target is kept only
//! where a record gives one (a template's is not read: no template holds one); a patrol
//! leader's banner is a plain white banner, as item stacks carry no patterns
//! (`n7k-plan.md`, section 10, "Items without components"), so that killing a leader
//! gives nobody bad omen; and the targets of each type are N9's, their goals held as
//! goals that never start (so that no target goal's draw of its interval is made), and
//! with them `AbstractIllager`'s refusal of a baby villager as a target and its allies
//! of `#illager_friends`.

use clustine_data::{entity_types, items};
use clustine_world::{BlockPos, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::template;
use super::{SpawnReason, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::mobs::ai::selector::flags;
use crate::mobs::types::tamable::Idle;
use crate::region::Random;
use crate::shown::DataShown;

/// The version of a raider's type state.
const VERSION: u8 = 1;

/// The synced data's index of `Raider.IS_CELEBRATING`.
pub(crate) const CELEBRATING_INDEX: u8 = 16;

/// The potions a witch drinks (`Potions`), by what each gives: water breathing,
/// fire resistance and swiftness for 3600 ticks, healing for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Potion {
    WaterBreathing,
    FireResistance,
    Healing,
    Swiftness,
}

/// What a raider keeps: `PatrollingMonster`'s and `Raider`'s saved fields and its
/// celebration, then each type's own (the witch's drinking, the pillager's charging,
/// the vindicator's name, the evoker's spell), left at their defaults by the others.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RaiderState {
    /// `patrolTarget` (`patrol_target`).
    pub(crate) patrol_target: Option<BlockPos>,
    /// `patrolLeader` (`PatrolLeader`).
    pub(crate) patrol_leader: bool,
    /// `patrolling` (`Patrolling`).
    pub(crate) patrolling: bool,
    /// `wave` (`Wave`).
    pub(crate) wave: i32,
    /// `canJoinRaid` (`CanJoinRaid`).
    pub(crate) can_join_raid: bool,
    /// `IS_CELEBRATING`, synced: set only by a raid's celebration (N10).
    pub(crate) celebrating: bool,
    /// The witch's potion while it drinks (`DATA_USING_ITEM`, synced at 17): what the
    /// potion in its hand would carry, which the stack cannot.
    pub(crate) drinking: Option<Potion>,
    /// The witch's `usingTime`: the drinking's ticks left.
    pub(crate) using_time: i32,
    /// The pillager's `IS_CHARGING_CROSSBOW`, synced at 17: set only by its attack (N9).
    pub(crate) charging: bool,
    /// The vindicator's `isJohnny` (`Johnny`).
    pub(crate) johnny: bool,
    /// The evoker's `spellCastingTickCount` (`SpellTicks`).
    pub(crate) spell_ticks: i32,
}

/// A raider's state from its type state; a record of nothing, or one this build cannot
/// read, as a fresh raider's.
pub(crate) fn state_of(mob: &Mob) -> RaiderState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return RaiderState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

pub(crate) fn put_state(mob: &mut Mob, own: &RaiderState) {
    mob.state = if *own == RaiderState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

/// Changes a raider's state by `change`, writing it back only where it changed.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut RaiderState)) {
    let before = state_of(mob);
    let mut own = before.clone();
    change(&mut own);
    if own != before {
        put_state(mob, &own);
    }
}

/// `Raider.finalizeSpawn` and `PatrollingMonster.finalizeSpawn`, before `Mob`'s:
/// `setCanJoinRaid(!(witch && NATURAL))`; then, unless the reason is a patrol's, an
/// event's or a structure's, one draw of the level's against 0.06, made before the type
/// is asked whether it can lead (`canBeLeader`: the witch cannot), and a leader wears
/// its banner (drop chance 2: always dropped, never worn out). A patrol's member
/// patrols (no patrol is made before N9).
pub(crate) fn finalize_spawn(kind: u16, spawn: &mut Spawning<'_>, can_be_leader: bool) {
    let witch = i32::from(kind) == entity_types::WITCH;
    let mut own = state_of(spawn.mob);
    own.can_join_raid = !(witch && spawn.reason == SpawnReason::Natural);
    if spawn.reason != SpawnReason::Structure && spawn.level.next_float() < 0.06 && can_be_leader {
        own.patrol_leader = true;
    }
    if own.patrol_leader {
        let body = &mut spawn.mob.body;
        body.equipment.slots[usize::from(slots::HEAD)] = Some(ItemStack {
            item: items::WHITE_BANNER,
            count: 1,
        });
        let mut chances = body.equipment.drop_chances.unwrap_or([0.085; 8]);
        chances[usize::from(slots::HEAD)] = 2.0;
        body.equipment.drop_chances = Some(chances);
    }
    put_state(spawn.mob, &own);
}

/// The goals `PatrollingMonster.registerGoals` and `Raider.registerGoals` add before
/// the type's own, each holding its place without starting: `LongDistancePatrolGoal`
/// (4), which only a patrol's member starts, and the raid's `ObtainRaidLeaderBannerGoal`
/// (1), `PathfindToRaidGoal` (3), `RaiderMoveThroughVillageGoal` (4) and
/// `RaiderCelebration` (5), which only a raid's.
pub(crate) fn register_raider_goals(setup: &mut Setup) {
    setup.add_goal(4, Idle { flags: flags::MOVE });
    setup.add_goal(1, Idle { flags: flags::MOVE });
    setup.add_goal(3, Idle { flags: flags::MOVE });
    setup.add_goal(4, Idle { flags: flags::MOVE });
    setup.add_goal(5, Idle { flags: flags::MOVE });
}

/// Target goals held in their places, N9's, one at each of `priorities`.
pub(crate) fn register_targets(setup: &mut Setup, priorities: &[i32]) {
    for &priority in priorities {
        setup.add_target(
            priority,
            Idle {
                flags: flags::TARGET,
            },
        );
    }
}

/// `Raider.removeWhenFarAway(distance)`: with no raid, `PatrollingMonster`'s: a mob not
/// patrolling may go, a patrolling one only beyond 128 blocks.
pub(crate) fn remove_when_far_away(mob: &Mob, distance: f64) -> bool {
    !state_of(mob).patrolling || distance > 16_384.0
}

/// What the raider layers read of a structure template's tag
/// (`PatrollingMonster.readAdditionalSaveData`, `Raider.readAdditionalSaveData`) and,
/// through `read`, what the type's own reads.
pub(crate) fn from_template(mob: &mut Mob, tag: &[u8], read: impl FnOnce(&mut RaiderState)) {
    update(mob, |own| {
        own.patrol_leader = template::boolean(tag, "PatrolLeader").unwrap_or(false);
        own.patrolling = template::boolean(tag, "Patrolling").unwrap_or(false);
        own.wave = template::int(tag, "Wave").unwrap_or(0);
        own.can_join_raid = template::boolean(tag, "CanJoinRaid").unwrap_or(false);
        read(own);
    });
}

/// `Raider.IS_CELEBRATING` as clients are sent it.
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let celebrating = state_of(mob).celebrating;
    vec![(
        CELEBRATING_INDEX,
        DataShown::Boolean(celebrating),
        !celebrating,
    )]
}

/// `Mob.getBaseExperienceReward()` of a mob whose `xpReward` is `reward` (5 for a
/// monster, 10 for a guardian): one to three more for each slot but the saddle that
/// holds something it does not always drop, drawn from its own random in
/// `EquipmentSlot.VALUES`'s order.
pub(crate) fn experience(reward: i32, mob: &Mob, draws: &mut Random) -> i32 {
    let mut total = reward;
    let equipment = &mob.body.equipment;
    for slot in 0..usize::from(slots::SADDLE) {
        if equipment.slots[slot].is_some() && equipment.drop_chance(slot) <= 1.0 {
            total += 1 + draws.next_int(3) as i32;
        }
    }
    total
}

/// `AbstractIllager.IllagerArmPose`: how a client draws an illager's arms, which it
/// derives from what it is sent (nothing of it is sent itself).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "what a client derives; the server only keeps what it derives it from"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArmPose {
    Crossed,
    Attacking,
    Spellcasting,
    CrossbowHold,
    CrossbowCharge,
    Celebrating,
    Neutral,
}

/// `getArmPose()` of an illager of the type `kind`, where it is aggressive as
/// `aggressive` says (Mob's flag, N9's: never yet): the pillager's by its crossbow, the
/// vindicator's attacking or celebrating, the evoker's casting or celebrating, else
/// `AbstractIllager`'s crossed arms.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "what a client derives; the server only keeps what it derives it from"
    )
)]
pub(crate) fn arm_pose(kind: u16, mob: &Mob, aggressive: bool) -> ArmPose {
    let own = state_of(mob);
    let holds_crossbow = [slots::MAIN_HAND, slots::OFF_HAND].iter().any(|&slot| {
        mob.body.equipment.slots[usize::from(slot)]
            .is_some_and(|stack| stack.item == items::CROSSBOW)
    });
    match i32::from(kind) {
        entity_types::PILLAGER => {
            if own.charging {
                ArmPose::CrossbowCharge
            } else if holds_crossbow {
                ArmPose::CrossbowHold
            } else if aggressive {
                ArmPose::Attacking
            } else {
                ArmPose::Neutral
            }
        }
        entity_types::VINDICATOR if aggressive => ArmPose::Attacking,
        entity_types::EVOKER if own.spell_ticks > 0 => ArmPose::Spellcasting,
        entity_types::VINDICATOR | entity_types::EVOKER if own.celebrating => ArmPose::Celebrating,
        _ => ArmPose::Crossed,
    }
}
