//! The villager's own behaviours that switch its activity and that its babies play by
//! (`docs/groundwork/n10-brief.md`, cited as V, sections 1.3 and 1.5;
//! `n10-plan.md`, section 5): its panic's trigger and its calming down, the raid's
//! status set and reset (`SetRaidStatus`, `ResetRaidStatus`), its schedule's update,
//! and play (`PlayTagWithOtherKids`, `JumpOnBed`). Its walks and its sleep are
//! `rest.rs`'s.
//!
//! An activity a behaviour changes (`setActiveActivityIfPossible`,
//! `updateActivityFromSchedule`) is asked of the brain through
//! [`super::super::Memories::requested`], which makes it active as the behaviour
//! returns.
//!
//! **Raids** do not exist yet (N10d): the rules that ask for one (`getRaidAt`) find none,
//! and make their draws.

use clustine_blocks::data::in_tag;
use clustine_data::entity_types;
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use super::super::sensors::find;
use super::super::{
    Activity, Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Status, Timed, Value,
};
use super::{entity_of, tracking, walk, walk_to_point};
use crate::mobs::Mob;
use crate::mobs::ai::{controls, random_pos};
use crate::mobs::brain::Tracker;
use crate::mobs::types::villager;

/// `isHurt(villager)`: `hurt_by` holds something.
fn is_hurt(cx: &Cx<'_, '_, '_>) -> bool {
    cx.memories.check(MemoryKind::HurtBy, Status::Present)
}

/// `hasHostile(villager)`: `nearest_hostile` holds something.
fn has_hostile(cx: &Cx<'_, '_, '_>) -> bool {
    cx.memories
        .check(MemoryKind::NearestHostile, Status::Present)
}

/// `VillagerPanicTrigger` (a `Behavior` of no entry condition, its duration 60): while
/// hurt or near a hostile, the panic made active, and as it starts, where the panic is
/// not active already, its walk, look, path and the targets of breeding and interaction
/// forgotten. Its tick asks for a golem every hundredth game tick
/// (`Villager.spawnGolemIfNeeded(level, time, 3)`), which is part V3's (golem summoning,
/// `docs/groundwork/n10-plan.md`, section 7) and is called through
/// [`spawn_golem_if_needed`].
pub(crate) struct VillagerPanicTrigger;

/// [`VillagerPanicTrigger`], boxed.
pub(crate) fn panic_trigger() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(VillagerPanicTrigger))
}

/// `Villager.spawnGolemIfNeeded(level, time, 3)` of a panicking villager: the golem
/// summoning of part V3, which this hook reaches once that part lands; nothing until then.
fn spawn_golem_if_needed(cx: &mut Cx<'_, '_, '_>, needed: usize) {
    let _ = (cx, needed);
}

impl Timed for VillagerPanicTrigger {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[]
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        is_hurt(cx) || has_hostile(cx)
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if !(is_hurt(cx) || has_hostile(cx)) {
            return;
        }
        if !cx.memories.active.contains(&Activity::Panic) {
            for kind in [
                MemoryKind::Path,
                MemoryKind::WalkTarget,
                MemoryKind::LookTarget,
                MemoryKind::BreedTarget,
                MemoryKind::InteractionTarget,
            ] {
                cx.memories.erase(kind);
            }
        }
        cx.memories.requested.push(Activity::Panic);
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if cx.time % 100 == 0 {
            spawn_golem_if_needed(cx, 3);
        }
    }
}

/// `SetRaidStatus.create()` (core 0, after the panic trigger; V 1.3): a draw of the
/// level's `nextInt(20)` every tick it is tried, and at 0, a raid near (`getRaidAt`)
/// making the raid (its first wave out and not between waves) or the pre-raid both the
/// default and the active activity. No raid is known yet: whatever it draws, nothing
/// follows, and it does not fire.
pub(crate) struct SetRaidStatus;

/// [`SetRaidStatus`], boxed.
pub(crate) fn set_raid_status() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(SetRaidStatus))
}

impl Shot for SetRaidStatus {
    fn required(&self) -> &[MemoryKind] {
        &[]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if cx.ai.tick.draws.next_int(20) != 0 {
            return false;
        }
        // `level.getRaidAt(blockPosition())`: none until raids are kept (N10d).
        false
    }
}

/// `ResetRaidStatus.create()` (pre-raid and raid 99; V 1.3): a draw of the level's
/// `nextInt(20)` every tick it is tried, and at 0, where no raid is near or it stopped
/// or was lost, the idle the default again and the schedule's activity
/// (`updateActivityFromSchedule`); it fires then.
pub(crate) struct ResetRaidStatus {
    /// `brain.updateActivityFromSchedule(…)`, the type's.
    pub(crate) update: fn(&mut Cx<'_, '_, '_>),
}

/// [`ResetRaidStatus`], boxed.
pub(crate) fn reset_raid_status(update: fn(&mut Cx<'_, '_, '_>)) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(ResetRaidStatus { update }))
}

impl Shot for ResetRaidStatus {
    fn required(&self) -> &[MemoryKind] {
        &[]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if cx.ai.tick.draws.next_int(20) != 0 {
            return false;
        }
        // No raid is near (none is kept until N10d): the default is idle again, which
        // it already is, as no raid ever made it another.
        (self.update)(cx);
        true
    }
}

/// `VillagerCalmDown.SAFE_DISTANCE_FROM_DANGER`, squared.
const SAFE_DISTANCE_SQR: f64 = 36.0;

/// `VillagerCalmDown.create()`: unless hurt, near a hostile, or within 6 of what hurt it
/// last, `hurt_by` and `hurt_by_entity` forgotten and the activity the schedule's
/// ([`VillagerCalmDown::update`]). It fires every time. What hurt it that the region's
/// view no longer has is taken as far off.
pub(crate) struct VillagerCalmDown {
    /// `brain.updateActivityFromSchedule(…)`, the type's.
    pub(crate) update: fn(&mut Cx<'_, '_, '_>),
}

/// [`VillagerCalmDown`], boxed.
pub(crate) fn calm_down(update: fn(&mut Cx<'_, '_, '_>)) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(VillagerCalmDown { update }))
}

impl Shot for VillagerCalmDown {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::HurtBy,
            MemoryKind::HurtByEntity,
            MemoryKind::NearestHostile,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let position = cx.ai.position();
        let near_attacker = entity_of(cx, MemoryKind::HurtByEntity)
            .and_then(|key| find(cx.ai, key))
            .is_some_and(|looked| looked.distance_sqr(position) <= SAFE_DISTANCE_SQR);
        let panicking = is_hurt(cx) || has_hostile(cx) || near_attacker;
        if !panicking {
            cx.memories.erase(MemoryKind::HurtBy);
            cx.memories.erase(MemoryKind::HurtByEntity);
            (self.update)(cx);
        }
        true
    }
}

/// `UpdateActivityFromSchedule.create()`: `brain.updateActivityFromSchedule(…)` each
/// tick its activity is active (the type's [`Self::update`], which acts at most every 21
/// game ticks); it fires every time.
pub(crate) struct UpdateActivityFromSchedule {
    pub(crate) update: fn(&mut Cx<'_, '_, '_>),
}

/// [`UpdateActivityFromSchedule`], boxed.
pub(crate) fn from_schedule(update: fn(&mut Cx<'_, '_, '_>)) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(UpdateActivityFromSchedule { update }))
}

impl Shot for UpdateActivityFromSchedule {
    fn required(&self) -> &[MemoryKind] {
        &[]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        (self.update)(cx);
        true
    }
}

// ------------------------------------------------------------------------------------
// Play.

/// `PlayTagWithOtherKids`' speed of a chase and of a flight.
const TAG_SPEED: f32 = 0.6;

/// `MAX_CHASERS_PER_TARGET`.
const MOST_CHASERS: usize = 5;

/// Whom the villager `key` chases (`interaction_target`), as its record shows it to its
/// neighbours (`VillagerState.shown`, written at the end of each of its ticks, so that a
/// copy across a line shows it too); none for a mob that is no villager.
fn chasing(cx: &Cx<'_, '_, '_>, key: EntityId) -> Option<EntityId> {
    let seen = cx.ai.tick.world.entity(key)?;
    if i32::from(seen.entity.kind) != entity_types::VILLAGER {
        return None;
    }
    let mob = Mob::decode(&seen.entity.data).ok()?;
    villager::state_of(&mob).1.shown.interaction_target
}

/// `PlayTagWithOtherKids.create()` (play 5; V 1.5): with babies in view and no walk
/// target, at the level's `nextInt(10) == 0`: where a baby chases it, it runs into the
/// village (up to ten of `LandRandomPos.getPos(mob, 20, 8)` until one is in a village);
/// else it chases the least chased of the babies being chased by at most five, or else
/// the first baby it sees (its interaction and look target, walked to within 1). It fires
/// at the draw's 0.
///
/// **Named**: the game counts the chasers in a hash map keyed by the chased entity, whose
/// order decides between equals and differs between runs; here equals are taken in the
/// order of their keys.
pub(crate) struct PlayTagWithOtherKids;

/// [`PlayTagWithOtherKids`], boxed.
pub(crate) fn play_tag_with_other_kids() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(PlayTagWithOtherKids))
}

impl PlayTagWithOtherKids {
    /// `chaseKid`: the kid its interaction target and look target (at its eyes), and a
    /// walk to it (at its feet) within 1.
    fn chase(cx: &mut Cx<'_, '_, '_>, kid: EntityId) {
        cx.memories
            .set(MemoryKind::InteractionTarget, Value::Entity(kid));
        cx.memories
            .set(MemoryKind::LookTarget, Value::Tracker(tracking(kid, true)));
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(tracking(kid, false), TAG_SPEED, 1),
        );
    }
}

impl Shot for PlayTagWithOtherKids {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::VisibleVillagerBabies,
            MemoryKind::WalkTarget,
            MemoryKind::LookTarget,
            MemoryKind::InteractionTarget,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(Value::Entities(babies)) = cx.memories.get(MemoryKind::VisibleVillagerBabies)
        else {
            return false;
        };
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::InteractionTarget, Status::Registered)
        {
            return false;
        }
        if cx.ai.tick.draws.next_int(10) != 0 {
            return false;
        }
        let own = cx.ai.tick.key;
        if babies.iter().any(|&kid| chasing(cx, kid) == Some(own)) {
            for _ in 0..10 {
                let Some(point) = random_pos::land_pos(cx.ai, 20, 8) else {
                    continue;
                };
                if cx
                    .ai
                    .tick
                    .world
                    .is_village(crate::physics::containing(point))
                {
                    cx.memories
                        .set(MemoryKind::WalkTarget, walk_to_point(point, TAG_SPEED, 0));
                    break;
                }
            }
            return true;
        }
        // `checkHowManyChasersEachFriendHas`: by whom each chasing baby chases.
        let mut chasers: Vec<(EntityId, usize)> = Vec::new();
        for &kid in &babies {
            let Some(chased) = chasing(cx, kid) else {
                continue;
            };
            match chasers.binary_search_by_key(&chased, |(key, _)| *key) {
                Ok(at) => chasers[at].1 += 1,
                Err(at) => chasers.insert(at, (chased, 1)),
            }
        }
        // `sorted(comparingInt(count))`, stable, then the first of at most five.
        chasers.sort_by_key(|(_, count)| *count);
        let chased = chasers
            .into_iter()
            .find(|(_, count)| *count > 0 && *count <= MOST_CHASERS)
            .map(|(key, _)| key);
        if let Some(kid) = chased.or_else(|| babies.first().copied()) {
            Self::chase(cx, kid);
        }
        true
    }
}

/// `JumpOnBed`'s time to reach the bed, and its cooldown between jumps.
const TIME_TO_REACH_BED: i32 = 100;
const JUMP_COOLDOWN: i32 = 5;

/// What `JumpOnBed` keeps between ticks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Jumping {
    target: Option<BlockPos>,
    time_to_reach: i32,
    jumps: i32,
    cooldown: i32,
}

/// `JumpOnBed(speed)` (play and idle; a `Behavior` of 60 to 60 ticks, never timing out;
/// V 1.5): a baby on or over a bed, or with a nearest bed, walks to that bed (100 ticks
/// to reach it) and jumps on it three to six times (`3 + nextInt(4)` of the level's),
/// five ticks apart, while it stays a bed babies jump on.
pub(crate) struct JumpOnBed {
    pub(crate) speed: f32,
}

/// [`JumpOnBed`], boxed.
pub(crate) fn jump_on_bed(speed: f32) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(JumpOnBed { speed }))
}

impl JumpOnBed {
    /// `isJumpable(pos)`: a bed babies jump on.
    fn jumpable(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
        in_tag(
            cx.ai.tick.world.state(pos),
            "minecraft:villager_babies_can_jump_on_bed",
        )
    }

    /// `onOrOverBed`: the block below its feet or its feet's own is one.
    fn on_or_over_bed(cx: &Cx<'_, '_, '_>) -> bool {
        let at = cx.ai.block_position();
        Self::jumpable(cx, BlockPos::new(at.x, at.y - 1, at.z)) || Self::jumpable(cx, at)
    }

    /// `getNearestBed`: the `nearest_bed` memory.
    fn nearest_bed(cx: &Cx<'_, '_, '_>) -> Option<BlockPos> {
        match cx.memories.get(MemoryKind::NearestBed)? {
            Value::Block(pos) => Some(pos),
            _ => None,
        }
    }
}

impl Timed for JumpOnBed {
    type State = Jumping;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::NearestBed, Status::Present),
            (MemoryKind::WalkTarget, Status::Absent),
        ]
    }

    fn extra_start(&self, _: &mut Jumping, cx: &mut Cx<'_, '_, '_>) -> bool {
        let tick = &*cx.ai.tick;
        tick.mob_type.is_baby(&tick.mob)
            && (Self::on_or_over_bed(cx) || Self::nearest_bed(cx).is_some())
    }

    fn can_still_use(&self, state: &mut Jumping, cx: &mut Cx<'_, '_, '_>) -> bool {
        let tick = &*cx.ai.tick;
        if !tick.mob_type.is_baby(&tick.mob) {
            return false;
        }
        let Some(target) = state.target else {
            return false;
        };
        let over = Self::on_or_over_bed(cx);
        let tired_of_walking = !over && state.time_to_reach <= 0;
        let tired_of_jumping = over && state.jumps <= 0;
        Self::jumpable(cx, target) && !tired_of_walking && !tired_of_jumping
    }

    fn timed_out(&self, _: &Jumping, _: &Cx<'_, '_, '_>, _: i64) -> bool {
        false
    }

    fn start(&self, state: &mut Jumping, cx: &mut Cx<'_, '_, '_>) {
        let Some(bed) = Self::nearest_bed(cx) else {
            return;
        };
        state.target = Some(bed);
        state.time_to_reach = TIME_TO_REACH_BED;
        state.jumps = 3 + cx.ai.tick.draws.next_int(4) as i32;
        state.cooldown = 0;
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(Tracker::Block(bed), self.speed, 0),
        );
    }

    fn tick(&self, state: &mut Jumping, cx: &mut Cx<'_, '_, '_>) {
        if !Self::on_or_over_bed(cx) {
            state.time_to_reach -= 1;
            return;
        }
        if state.cooldown > 0 {
            state.cooldown -= 1;
            return;
        }
        // `onBedSurface`: its feet's own block is one.
        if Self::jumpable(cx, cx.ai.block_position()) {
            controls::jump(cx.ai);
            state.jumps -= 1;
            state.cooldown = JUMP_COOLDOWN;
        }
    }

    fn stop(&self, state: &mut Jumping, _: &mut Cx<'_, '_, '_>) {
        *state = Jumping::default();
    }
}
