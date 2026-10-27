//! What the witch and the illagers share (`n7k-brief.md` 3.1, 3.6 and 3.7): the layers
//! `PatrollingMonster`, `Raider`, `AbstractIllager` and `SpellcasterIllager` add to a
//! monster, and the one type state the four types keep, each its own part of it.
//!
//! Named differences: no raid exists before N10, so the raid's goals hold their places
//! as goals that never start, a raider never joins a raid nor celebrates, and its
//! `Wave` and `CanJoinRaid` are kept and never read; a patrol's target from a structure
//! template is not read (no template holds one). Patrols, their goal and the leader's
//! ominous banner are `crate::mobs::ai::patrol`'s (N9's S11). The targets of each type,
//! with `AbstractIllager`'s refusal of
//! a baby villager, are `crate::mobs::ai::targets`'; its allies of `#illager_friends`
//! are never asked of a target any of them takes.

use clustine_data::{entity_types, items};
use clustine_world::{BlockPos, TypeState};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::evoker::EvokerState;
use super::pillager::PillagerState;
use super::template;
use super::{SpawnReason, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::selector::flags;
use crate::mobs::ai::{Setup, patrol};
use crate::mobs::types::tamable::Idle;
use crate::region::Random;
use crate::shown::DataShown;

/// The version of a raider's type state: 2 since the shared part and the type's own
/// were split (N9f-3). A version 1 record, which held every type's fields in one
/// struct, the shared ones first, is read into the shared part; its type's own fields
/// (a witch's drinking, a pillager's charging, a vindicator's `Johnny`, an evoker's
/// spell's ticks) are lost, each back at its default.
///
/// Version 3 adds the raid a raider is of and its ticks outside it to the shared part
/// (`docs/groundwork/n10-plan.md`, section 9.3); a version 2 record's shared part is read
/// in its shape, without them.
const VERSION: u8 = 3;

/// The version before the raid was kept.
const VERSION_BEFORE_RAIDS: u8 = 2;

/// The version before the split.
const VERSION_SHARED_ONLY: u8 = 1;

/// The shared part as versions 1 and 2 kept it.
#[derive(Deserialize)]
struct SharedBeforeRaids {
    patrol_target: Option<BlockPos>,
    patrol_leader: bool,
    patrolling: bool,
    wave: i32,
    can_join_raid: bool,
    celebrating: bool,
}

impl From<SharedBeforeRaids> for RaiderState {
    fn from(old: SharedBeforeRaids) -> Self {
        Self {
            patrol_target: old.patrol_target,
            patrol_leader: old.patrol_leader,
            patrolling: old.patrolling,
            wave: old.wave,
            can_join_raid: old.can_join_raid,
            celebrating: old.celebrating,
            raid: None,
            ticks_outside_raid: 0,
        }
    }
}

/// The synced data's index of `Raider.IS_CELEBRATING`.
pub(crate) const CELEBRATING_INDEX: u8 = 16;

/// What every raider keeps: `PatrollingMonster`'s and `Raider`'s saved fields and its
/// celebration. Each type's own is a struct in its own module (`WitchState`,
/// `PillagerState`, `VindicatorState`, `EvokerState`), kept after this in the same type
/// state, so that the parts that fill each type never touch this file.
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
    /// The raid it is of (`currentRaid`, saved by the game as `RaidId`), by the raid's id
    /// (`docs/groundwork/n10-plan.md`, section 9.3).
    pub(crate) raid: Option<clustine_world::EntityId>,
    /// `ticksOutsideRaid`: counted while it has a raid and is outside a village.
    pub(crate) ticks_outside_raid: i32,
}

/// What a type's own part of a raider's state must be: kept beside the shared part,
/// compared to know whether it changed, and left out of the record at its default.
pub(crate) trait OwnState:
    Default + Clone + PartialEq + Serialize + DeserializeOwned
{
}

impl<T: Default + Clone + PartialEq + Serialize + DeserializeOwned> OwnState for T {}

/// The two parts of a raider's type state as they are kept: the shared part decoded,
/// the type's own still in its bytes (none at its default), so that a change of one
/// part need not know the other's type.
fn parts(mob: &Mob) -> (RaiderState, &[u8]) {
    if mob.state.bytes.is_empty() {
        return (RaiderState::default(), &[]);
    }
    match mob.state.version {
        VERSION => postcard::take_from_bytes::<RaiderState>(&mob.state.bytes).unwrap_or_default(),
        VERSION_BEFORE_RAIDS => postcard::take_from_bytes::<SharedBeforeRaids>(&mob.state.bytes)
            .map(|(shared, own)| (RaiderState::from(shared), own))
            .unwrap_or_default(),
        // The shared fields lead a version 1 record; what follows them is in a shape
        // no longer read.
        VERSION_SHARED_ONLY => (
            postcard::take_from_bytes::<SharedBeforeRaids>(&mob.state.bytes)
                .map(|(shared, _)| RaiderState::from(shared))
                .unwrap_or_default(),
            &[],
        ),
        _ => (RaiderState::default(), &[]),
    }
}

/// Writes the two parts back: postcard's encoding of the shared part followed by the
/// own part's bytes, the record empty where both are at their defaults.
fn put_parts(mob: &mut Mob, shared: &RaiderState, own: &[u8]) {
    mob.state = if *shared == RaiderState::default() && own.is_empty() {
        TypeState::default()
    } else {
        let mut bytes = postcard::to_allocvec(shared).expect("a type state is encoded into memory");
        bytes.extend_from_slice(own);
        TypeState {
            version: VERSION,
            bytes,
        }
    };
}

/// The bytes of a type's own part: none at its default.
fn own_bytes<T: OwnState>(own: &T) -> Vec<u8> {
    if *own == T::default() {
        Vec::new()
    } else {
        postcard::to_allocvec(own).expect("a type state is encoded into memory")
    }
}

/// A raider's shared state from its type state; a record of nothing, or one this build
/// cannot read, as a fresh raider's.
pub(crate) fn state_of(mob: &Mob) -> RaiderState {
    parts(mob).0
}

/// A raider's own part, of the type `T` its type keeps; nothing, or what this build
/// cannot read, as a fresh one's.
pub(crate) fn own_of<T: OwnState>(mob: &Mob) -> T {
    let (_, own) = parts(mob);
    if own.is_empty() {
        return T::default();
    }
    postcard::from_bytes(own).unwrap_or_default()
}

/// Changes a raider's shared state by `change`, keeping its own part as it is and
/// writing back only where it changed.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut RaiderState)) {
    let (before, own) = parts(mob);
    let mut shared = before.clone();
    change(&mut shared);
    if shared != before {
        let own = own.to_vec();
        put_parts(mob, &shared, &own);
    }
}

/// Changes a raider's own part, of the type `T`, by `change`, keeping its shared part
/// as it is and writing back only where it changed.
pub(crate) fn update_own<T: OwnState>(mob: &mut Mob, change: impl FnOnce(&mut T)) {
    let before = own_of::<T>(mob);
    let mut own = before.clone();
    change(&mut own);
    if own != before {
        let shared = state_of(mob);
        put_parts(mob, &shared, &own_bytes(&own));
    }
}

/// `Raider.finalizeSpawn` and `PatrollingMonster.finalizeSpawn`, before `Mob`'s:
/// `setCanJoinRaid(!(witch && NATURAL))`; then, unless the reason is a patrol's, an
/// event's or a structure's, one draw of the level's against 0.06, made before the type
/// is asked whether it can lead (`canBeLeader`: the witch cannot), and a leader wears
/// the ominous banner (drop chance 2: always dropped, never worn out). A patrol's member
/// patrols.
pub(crate) fn finalize_spawn(kind: u16, spawn: &mut Spawning<'_>, can_be_leader: bool) {
    let witch = i32::from(kind) == entity_types::WITCH;
    let mut leads = false;
    update(spawn.mob, |own| {
        own.can_join_raid = !(witch && spawn.reason == SpawnReason::Natural);
        if !matches!(
            spawn.reason,
            SpawnReason::Structure | SpawnReason::Patrol | SpawnReason::Event
        ) && spawn.level.next_float() < 0.06
            && can_be_leader
        {
            own.patrol_leader = true;
        }
        leads = own.patrol_leader;
        if spawn.reason == SpawnReason::Patrol {
            own.patrolling = true;
        }
    });
    if leads {
        let body = &mut spawn.mob.body;
        body.equipment.slots[usize::from(slots::HEAD)] = Some(patrol::ominous_banner());
        let mut chances = body.equipment.drop_chances.unwrap_or([0.085; 8]);
        chances[usize::from(slots::HEAD)] = 2.0;
        body.equipment.drop_chances = Some(chances);
    }
}

/// The goals `PatrollingMonster.registerGoals` and `Raider.registerGoals` add before
/// the type's own: `LongDistancePatrolGoal` (4, `crate::mobs::ai::patrol`), and the
/// raid's `ObtainRaidLeaderBannerGoal` (1), `PathfindToRaidGoal` (3),
/// `RaiderMoveThroughVillageGoal` (4) and `RaiderCelebration` (5), each holding its
/// place without starting, as only a raid's would.
pub(crate) fn register_raider_goals(setup: &mut Setup) {
    setup.add_goal(4, patrol::PATROL_GOAL);
    setup.add_goal(1, Idle { flags: flags::MOVE });
    setup.add_goal(3, Idle { flags: flags::MOVE });
    setup.add_goal(4, Idle { flags: flags::MOVE });
    setup.add_goal(5, Idle { flags: flags::MOVE });
}

/// `Raider.removeWhenFarAway(distance)`: with no raid, `PatrollingMonster`'s: a mob not
/// patrolling may go, a patrolling one only beyond 128 blocks.
pub(crate) fn remove_when_far_away(mob: &Mob, distance: f64) -> bool {
    !state_of(mob).patrolling || distance > 16_384.0
}

/// What the raider layers read of a structure template's tag
/// (`PatrollingMonster.readAdditionalSaveData`, `Raider.readAdditionalSaveData`) and,
/// what the type's own reads it reads itself, after, with `update_own`.
pub(crate) fn from_template(mob: &mut Mob, tag: &[u8]) {
    update(mob, |own| {
        own.patrol_leader = template::boolean(tag, "PatrolLeader").unwrap_or(false);
        own.patrolling = template::boolean(tag, "Patrolling").unwrap_or(false);
        own.wave = template::int(tag, "Wave").unwrap_or(0);
        own.can_join_raid = template::boolean(tag, "CanJoinRaid").unwrap_or(false);
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
    let shared = state_of(mob);
    let holds_crossbow = [slots::MAIN_HAND, slots::OFF_HAND].iter().any(|&slot| {
        mob.body.equipment.slots[usize::from(slot)]
            .as_ref()
            .is_some_and(|stack| stack.item == items::CROSSBOW)
    });
    match i32::from(kind) {
        entity_types::PILLAGER => {
            if own_of::<PillagerState>(mob).charging {
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
        entity_types::EVOKER if own_of::<EvokerState>(mob).spell_ticks > 0 => ArmPose::Spellcasting,
        entity_types::VINDICATOR | entity_types::EVOKER if shared.celebrating => {
            ArmPose::Celebrating
        }
        _ => ArmPose::Crossed,
    }
}
