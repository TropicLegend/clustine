//! What a raider does of its raid (`Raider` and its goals; V 7.3,
//! `docs/groundwork/n10-plan.md`, section 9.7): the goals every raider adds
//! (`ObtainRaidLeaderBannerGoal` 1, `PathfindToRaidGoal` 3,
//! `RaiderMoveThroughVillageGoal` 4, `RaiderCelebration` 5), its `aiStep`'s joining of a
//! raid near it, the vindicator's door goals and the witch's healing of raiders, and the
//! word a raider sends its raid of itself ([`RaiderNews`]).
//!
//! A raider keeps its raid's id (`illagers::RaiderState::raid`) and reads the raid
//! through its view ([`RaidKnown`]): every field of the region's own raids, the table's
//! summary of another region's. What it changes of its raid is a [`RaiderNews`] its region
//! takes to the raid's record (`region/raid_tick.rs`), where the raid's region judges it
//! (section 9.3).
//!
//! Named differences: a raider recruits, and a leader's banner is picked up, only of the
//! region's own entities, not of copies; a raider whose raid is another region's knows no
//! wave of it and joins it only before its first wave (the table carries no wave count);
//! a leader of another region's raid is not known, so no banner is sought for one; a
//! picked banner is not given to the leader of another region's raid; the healer's
//! cooldown runs down whether or not the witch's raid is active; the witch's player
//! target is never held back for healing (`NearestAttackableWitchTargetGoal.canAttack`);
//! the vindicator breaks no door, as the world is easy (its goal draws and refuses);
//! the raiders' sets are walked in the box's order, where the game's are hash sets.

use clustine_data::{attributes, entity_types};
use clustine_world::poi::PoiType;
use clustine_world::raid::RaidStatus;
use clustine_world::{BlockPos, EntityData, EntityId, ItemStack};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::goals::{TargetRules, Targeting};
use super::maths::reduced_tick_delay;
use super::selector::{Goal, flags};
use super::sensing::{self, Class};
use super::targets::{Among, RAIDERS, find_nearest};
use super::{Ai, controls, navigation, patrol, random_pos};
use crate::api::RaidSeen;
use crate::mobs::Mob;
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::types::illagers;
use crate::physics::{self, Vec3};
use crate::region::pois::Occupancy;

/// `Raid`'s reach for its members: one 112 or more from its centre leaves it
/// (`distSqr >= 12544`).
pub(crate) const MEMBER_REACH_SQR: f64 = 12_544.0;

/// The idle time beyond which a raider may not join a raid and counts outside a village
/// (`noActionTime > 2400`).
pub(crate) const IDLE_LIMIT: i64 = 2400;

/// A raid as a raider's view knows it: what every region reads of it, whether it is the
/// region's own, and of its own raids its waves so far and the leader of the wave asked
/// of.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RaidKnown {
    pub(crate) seen: RaidSeen,
    pub(crate) own: bool,
    /// `getGroupsSpawned()`, where the raid is the region's own.
    pub(crate) groups_spawned: Option<i32>,
    /// `getLeader(wave)`, where the raid is the region's own.
    pub(crate) leader: Option<Option<EntityId>>,
}

impl RaidKnown {
    /// `isActive()`.
    pub(crate) fn is_active(&self) -> bool {
        self.seen.active
    }

    /// `isOver()`: won or lost.
    pub(crate) fn is_over(&self) -> bool {
        matches!(self.seen.status, RaidStatus::Victory | RaidStatus::Loss)
    }

    /// `isLoss()`.
    pub(crate) fn is_loss(&self) -> bool {
        self.seen.status == RaidStatus::Loss
    }

    /// The wave a raider joining now joins (`getGroupsSpawned()`): known of the region's
    /// own; of another region's only before its first wave, as none.
    fn joining_wave(&self) -> Option<i32> {
        match self.groups_spawned {
            Some(groups) => Some(groups),
            None => (!self.seen.first_wave_spawned).then_some(0),
        }
    }
}

/// What a raider tells its raid, which its region takes to the raid's record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RaiderNews {
    /// It joined the raid's wave `wave` (`joinRaid` of a raider already made), its state
    /// set in its own tick, with its health for the raid's total.
    Joined {
        raid: EntityId,
        wave: i32,
        health: f32,
    },
    /// It recruits the raider the news names (`PathfindToRaidGoal.recruitNearby`): the
    /// region sets that one's raid and wave and tells the raid.
    Recruit { raid: EntityId, wave: i32 },
    /// It is hurt (`Raider.hurtServer`'s `updateBossbar`, before the damage lands): its
    /// health for the bar.
    Hurt { raid: EntityId, health: f32 },
    /// It died (`Raider.die`), by a player's hand where `killer` names one.
    Died {
        raid: EntityId,
        killer: Option<Uuid>,
    },
    /// It left the raid (`updateRaiders`' judgement of a member another region holds).
    Left { raid: EntityId },
    /// It picked up the ominous banner and leads its wave (`Raider.pickUpItem`'s
    /// `setLeader`).
    Leads { raid: EntityId, wave: i32 },
}

/// `getCurrentRaid()` as the view knows it: none where it has none, or its raid is no
/// longer known.
pub(crate) fn current_raid(tick: &MobTick<'_>) -> Option<RaidKnown> {
    let state = illagers::state_of(&tick.mob);
    tick.world.raid_known(state.raid?, state.wave)
}

/// `hasActiveRaid()`.
pub(crate) fn has_active_raid(tick: &MobTick<'_>) -> bool {
    current_raid(tick).is_some_and(|raid| raid.is_active())
}

/// `noActionTime = 0` in its `aiStep`, before its step counts it on.
fn reset_idle_before_step(tick: &mut MobTick<'_>) {
    if let Some(since) = &mut tick.mob.body.idle.no_action_since {
        *since = tick.lived;
        tick.mob.body.idle_gained.gained = 0;
    }
}

/// `noActionTime = 0` in a goal, after its step counted it.
fn reset_idle_in_step(ai: &mut Ai<'_, '_>) {
    if let Some(since) = &mut ai.tick.mob.body.idle.no_action_since {
        *since = ai.tick.lived + 1;
        ai.tick.mob.body.idle_gained.gained = 0;
    }
}

/// `Raids.canJoinRaid(raider)`: alive, able to join, and not idle too long.
fn can_join(mob: &Mob, lived: u32) -> bool {
    !mob.body.vitals.dying()
        && illagers::state_of(mob).can_join_raid
        && mob.body.no_action_time(lived) <= IDLE_LIMIT
}

/// `Raider.aiStep`'s own part, before `PatrollingMonster`'s, of a living raider that may
/// join a raid: with no raid, every twentieth game tick, the raid `getRaidAt` finds where
/// it stands is joined on its current wave; with one, a target that is a player or an
/// iron golem keeps it from idling. Besides, a raider of a raid another region holds is
/// judged here as that raid's `updateRaiders` would judge it (section 9.3).
pub(crate) fn ai_step(tick: &mut MobTick<'_>) {
    if tick.mob.body.vitals.dying() {
        return;
    }
    let state = illagers::state_of(&tick.mob);
    if !state.can_join_raid {
        return;
    }
    let block = physics::containing(tick.living.body.position);
    match state.raid {
        None => {
            if tick.world.game_time().rem_euclid(20) != 0 {
                return;
            }
            let Some(seen) = tick.world.raid_at(block) else {
                return;
            };
            if !can_join(&tick.mob, tick.lived) {
                return;
            }
            let Some(known) = tick.world.raid_known(seen.id, 0) else {
                return;
            };
            let Some(wave) = known.joining_wave() else {
                return;
            };
            illagers::update(&mut tick.mob, |raider| {
                raider.raid = Some(seen.id);
                raider.wave = wave;
                raider.can_join_raid = true;
                raider.ticks_outside_raid = 0;
            });
            let health = tick.mob.body.vitals.health;
            tick.out.local.push(Local::Raider {
                key: None,
                news: RaiderNews::Joined {
                    raid: seen.id,
                    wave,
                    health,
                },
            });
        }
        Some(raid) => {
            if let Some(target) = tick.mob.body.combat.target {
                let player = tick.world.players().iter().any(|seen| seen.key == target);
                let golem = tick
                    .world
                    .entity(target)
                    .is_some_and(|seen| i32::from(seen.entity.kind) == entity_types::IRON_GOLEM);
                if player || golem {
                    reset_idle_before_step(tick);
                }
            }
            judged_from_afar(tick, raid, block);
        }
    }
}

/// `Raid.updateRaiders` of a member whose raid another region holds, by the rules its
/// own region can judge (section 9.3): every twentieth game tick of an ongoing, active
/// raid, it leaves at 112 or more from the centre, or after 30 passes idle outside a
/// village once it has lived 600 ticks.
fn judged_from_afar(tick: &mut MobTick<'_>, raid: EntityId, block: BlockPos) {
    if tick.world.game_time().rem_euclid(20) != 0 {
        return;
    }
    let state = illagers::state_of(&tick.mob);
    let Some(known) = tick.world.raid_known(raid, state.wave) else {
        return;
    };
    if known.own || known.seen.status != RaidStatus::Ongoing || !known.is_active() {
        return;
    }
    let leaves = if crate::region::pois::dist_sqr(known.seen.centre, block) >= MEMBER_REACH_SQR {
        true
    } else if tick.lived > 600 {
        let mut outside = state.ticks_outside_raid;
        if !tick.world.is_village(block) && tick.mob.body.no_action_time(tick.lived) > IDLE_LIMIT {
            outside += 1;
            illagers::update(&mut tick.mob, |raider| raider.ticks_outside_raid = outside);
        }
        outside >= 30
    } else {
        false
    };
    if leaves {
        illagers::update(&mut tick.mob, |raider| raider.raid = None);
        tick.out.local.push(Local::Raider {
            key: None,
            news: RaiderNews::Left { raid },
        });
    }
}

/// `Raid.getOminousBannerInstance` as an item lies: the banner, one of it.
fn is_ominous_banner(stack: &ItemStack) -> bool {
    *stack == patrol::ominous_banner()
}

/// The bottom middle of a block (`Vec3.atBottomCenterOf`).
fn bottom_centre(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// `BlockPos.closerToCenterThan(position, distance)`.
fn closer_to_centre(pos: BlockPos, position: Vec3, distance: f64) -> bool {
    let centre = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ];
    physics::distance_sqr(centre, position) < distance * distance
}

/// `Raider$ObtainRaidLeaderBannerGoal`'s state: the items found unreachable, until when
/// (`unreachableBannerCache`, by the item's key), and the banner pursued.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pursuit {
    unreachable: Vec<(EntityId, i64)>,
    pursued: Option<EntityId>,
}

/// `Raider$ObtainRaidLeaderBannerGoal`: a raider of an active raid that can lead, while
/// its wave has no living leader and it wears no banner, goes for an ominous banner lying
/// within its follow range (8 up and down) that it can reach, and picks it up within
/// 1.414 of it (`Raider.pickUpItem`). The banners it found it cannot reach it leaves be
/// for 600 game ticks.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ObtainRaidLeaderBannerGoal;

impl ObtainRaidLeaderBannerGoal {
    /// `cannotPickUpBanner()`.
    fn cannot_pick_up(ai: &Ai<'_, '_>) -> bool {
        let Some(raid) = current_raid(ai.tick).filter(RaidKnown::is_active) else {
            return true;
        };
        if raid.is_over() || !illagers::can_be_leader(ai.tick.kind) {
            return true;
        }
        let head = &ai.tick.mob.body.equipment.slots[usize::from(crate::living::slots::HEAD)];
        if head.as_ref().is_some_and(is_ominous_banner) {
            return true;
        }
        // A leader of another region's raid is not known: taken as living.
        match raid.leader {
            Some(leader) => leader.is_some_and(|leader| {
                ai.tick
                    .world
                    .entity(leader)
                    .is_some_and(|seen| sensing::Looked::of_seen(&seen).alive)
                    || leader == ai.tick.key
            }),
            None => true,
        }
    }

    /// The banners the goal may go for (`ALLOWED_ITEMS`): of the region's own, not held
    /// back, alive and the ominous banner, in the box's order.
    fn banners(ai: &Ai<'_, '_>) -> Vec<(EntityId, BlockPos)> {
        let follow = ai.attribute(attributes::FOLLOW_RANGE);
        let bounds = super::goals::ai_bounds(ai).inflate([follow, 8.0, follow]);
        ai.tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter(|seen| !seen.copied)
            .filter_map(|seen| match &seen.entity.data {
                EntityData::Item {
                    stack,
                    pick_up_delay: 0,
                    ..
                } if stack.count > 0 && is_ominous_banner(stack) => {
                    Some((seen.entity.key, physics::containing(seen.entity.position)))
                }
                _ => None,
            })
            .collect()
    }
}

impl Goal for ObtainRaidLeaderBannerGoal {
    type State = Pursuit;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Pursuit, ai: &mut Ai<'_, '_>) -> bool {
        if Self::cannot_pick_up(ai) {
            return false;
        }
        let now = ai.tick.world.game_time();
        let mut unreachable = Vec::new();
        for (item, at) in Self::banners(ai) {
            let until = state
                .unreachable
                .iter()
                .find(|(key, _)| *key == item)
                .map_or(i64::MIN, |(_, until)| *until);
            if now < until {
                unreachable.push((item, until));
                continue;
            }
            let made = navigation::create_path(ai, at, 1);
            let reaches = made
                .as_ref()
                .and_then(|made| made.path(ai))
                .is_some_and(|path| path.reached);
            if reaches {
                state.pursued = Some(item);
                return true;
            }
            unreachable.push((item, now + 600));
        }
        state.unreachable = unreachable;
        false
    }

    fn can_continue(&self, state: &mut Pursuit, ai: &mut Ai<'_, '_>) -> bool {
        let Some(item) = state.pursued else {
            return false;
        };
        if ai.tick.world.entity(item).is_none() || navigation::is_done(ai) {
            return false;
        }
        !Self::cannot_pick_up(ai)
    }

    /// `moveTo(pathToBanner, 1.15)`: the path to the banner, made again.
    fn start(&self, state: &mut Pursuit, ai: &mut Ai<'_, '_>) {
        let Some(at) = state
            .pursued
            .and_then(|item| ai.tick.world.entity(item))
            .map(|seen| physics::containing(seen.entity.position))
        else {
            return;
        };
        let made = navigation::create_path(ai, at, 1);
        navigation::move_to_path(ai, made, 1.149_999_976_158_142);
    }

    fn stop(&self, state: &mut Pursuit, _: &mut Ai<'_, '_>) {
        state.pursued = None;
    }

    fn tick(&self, state: &mut Pursuit, ai: &mut Ai<'_, '_>) {
        let Some(item) = state.pursued else {
            return;
        };
        let Some((position, stack)) =
            ai.tick
                .world
                .entity(item)
                .and_then(|seen| match &seen.entity.data {
                    EntityData::Item { stack, .. } => Some((seen.entity.position, stack.clone())),
                    _ => None,
                })
        else {
            return;
        };
        if physics::distance_sqr(position, ai.position()) < 1.414 * 1.414 {
            pick_up_banner(ai, item, stack);
        }
    }
}

/// `Raider.pickUpItem(level, item)` of the ominous banner by a raider of an active raid
/// whose wave has no leader: what it wore on its head dropped by its chance, the banner
/// worn, the item taken, and the raider its wave's leader.
fn pick_up_banner(ai: &mut Ai<'_, '_>, item: EntityId, stack: ItemStack) {
    let Some(raid) = current_raid(ai.tick).filter(RaidKnown::is_active) else {
        return;
    };
    if raid.leader.is_none_or(|leader| leader.is_some()) || !is_ominous_banner(&stack) {
        return;
    }
    let head = usize::from(crate::living::slots::HEAD);
    let worn = ai.tick.mob.body.equipment.slots[head].clone();
    let chance = f64::from(ai.tick.mob.body.equipment.drop_chance(head));
    if let Some(worn) = worn.filter(|worn| worn.count > 0)
        && f64::from((ai.tick.draws.next_float() - 0.1).max(0.0)) < chance
    {
        ai.tick.out.local.push(Local::Drop {
            stack: worn,
            at: ai.position(),
            pick_up_delay: 10,
        });
    }
    ai.tick.mob.body.equipment.slots[head] = Some(stack.clone());
    ai.tick.out.local.push(Local::TakeItem {
        item,
        count: stack.count,
    });
    let wave = illagers::state_of(&ai.tick.mob).wave;
    illagers::update(&mut ai.tick.mob, |raider| raider.patrol_leader = true);
    ai.tick.out.local.push(Local::Raider {
        key: None,
        news: RaiderNews::Leads {
            raid: raid.seen.id,
            wave,
        },
    });
}

/// `PathfindToRaidGoal`'s state: the tick of its next recruitment (`recruitmentTick`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Recruiting {
    next: u32,
}

/// `PathfindToRaidGoal`: a raider of an active raid not yet over, with no target and no
/// rider steering it, outside a village, walks towards the raid's centre
/// (`DefaultRandomPos.getPosTowards(15, 4, centre, π/2)` whenever its path is done), and
/// every twenty of its ticks recruits the raiders within 16 of it that have no active raid
/// and may join (`recruitNearby`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PathfindToRaidGoal;

impl PathfindToRaidGoal {
    fn going(ai: &Ai<'_, '_>) -> bool {
        current_raid(ai.tick).is_some_and(|raid| raid.is_active() && !raid.is_over())
            && !ai.tick.world.is_village(ai.block_position())
    }
}

impl Goal for PathfindToRaidGoal {
    type State = Recruiting;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut Recruiting, ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.mob.body.combat.target.is_none() && !ai.tick.controlled_by_mob && Self::going(ai)
    }

    fn can_continue(&self, _: &mut Recruiting, ai: &mut Ai<'_, '_>) -> bool {
        Self::going(ai)
    }

    fn tick(&self, state: &mut Recruiting, ai: &mut Ai<'_, '_>) {
        let Some(raid) = current_raid(ai.tick).filter(RaidKnown::is_active) else {
            return;
        };
        let now = ai.tick_count();
        if now > state.next {
            state.next = now + 20;
            recruit_nearby(ai, &raid);
        }
        if navigation::in_progress(ai) {
            return;
        }
        let centre = bottom_centre(raid.seen.centre);
        if let Some(at) =
            random_pos::default_pos_towards(ai, 15, 4, centre, std::f64::consts::FRAC_PI_2)
        {
            navigation::move_to(ai, at, 1.0);
        }
    }
}

/// `PathfindToRaidGoal.recruitNearby(raid)`: of an active raid, every raider of the
/// region's own within 16 of its box with no active raid that may join is asked to join
/// on the raid's current wave.
fn recruit_nearby(ai: &mut Ai<'_, '_>, raid: &RaidKnown) {
    let Some(wave) = raid.joining_wave() else {
        return;
    };
    let bounds = super::goals::ai_bounds(ai).inflate([16.0, 16.0, 16.0]);
    let recruits: Vec<EntityId> = ai
        .tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied && RAIDERS.contains(&seen.entity.kind))
        .filter(|seen| seen.entity.key != ai.tick.key)
        .filter(|seen| {
            Mob::decode(&seen.entity.data).is_ok_and(|mob| {
                let active = illagers::state_of(&mob)
                    .raid
                    .and_then(|id| ai.tick.world.raid_known(id, 0))
                    .is_some_and(|known| known.is_active());
                !active && can_join(&mob, seen.entity.lived)
            })
        })
        .map(|seen| seen.entity.key)
        .collect();
    for key in recruits {
        ai.tick.out.local.push(Local::Raider {
            key: Some(key),
            news: RaiderNews::Recruit {
                raid: raid.seen.id,
                wave,
            },
        });
    }
}

/// `Raider$RaiderMoveThroughVillageGoal`'s state: the home it goes to (`poiPos`), the
/// last homes it reached (`visited`, at most three) and whether it found no way on
/// (`stuck`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Through {
    poi: Option<BlockPos>,
    visited: Vec<BlockPos>,
    stuck: bool,
}

/// `Raider$RaiderMoveThroughVillageGoal(raider, 1.05, 1)`: a raider of an active raid not
/// yet over and with no target goes to a home (a bed's point of interest) within 48 of it
/// that it has not lately reached, chosen at random of its own stream
/// (`PoiManager.getRandom`), and walks on towards it until within a block and its width.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RaiderMoveThroughVillageGoal;

/// Its speed.
const THROUGH_SPEED: f64 = 1.05;
/// `distanceToPoi`.
const DISTANCE_TO_POI: f64 = 1.0;

impl RaiderMoveThroughVillageGoal {
    /// `hasSuitablePoi()`: a home within 48 of the raider's block, the records shuffled of
    /// its own stream as `Util.toShuffledList` shuffles, the first not visited.
    fn suitable(state: &mut Through, ai: &mut Ai<'_, '_>) -> bool {
        let mut found = ai.tick.world.pois_in_range(
            ai.block_position(),
            48,
            &|kind| kind == PoiType::Home,
            Occupancy::Any,
        );
        for i in (2..=found.len()).rev() {
            let j = ai.tick.draws.next_int(i as u32) as usize;
            found.swap(i - 1, j);
        }
        let Some(record) = found
            .into_iter()
            .find(|record| !state.visited.contains(&record.pos))
        else {
            return false;
        };
        state.poi = Some(record.pos);
        true
    }
}

impl Goal for RaiderMoveThroughVillageGoal {
    type State = Through;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Through, ai: &mut Ai<'_, '_>) -> bool {
        // `updateVisited`: the oldest forgotten beyond two.
        if state.visited.len() > 2 {
            state.visited.remove(0);
        }
        let valid = current_raid(ai.tick).is_some_and(|raid| raid.is_active() && !raid.is_over());
        valid && Self::suitable(state, ai) && ai.tick.mob.body.combat.target.is_none()
    }

    fn can_continue(&self, state: &mut Through, ai: &mut Ai<'_, '_>) -> bool {
        if navigation::is_done(ai) || ai.tick.mob.body.combat.target.is_some() {
            return false;
        }
        let Some(poi) = state.poi else {
            return false;
        };
        let reach = f64::from(ai.width() + DISTANCE_TO_POI as f32);
        !closer_to_centre(poi, ai.position(), reach) && !state.stuck
    }

    fn start(&self, state: &mut Through, ai: &mut Ai<'_, '_>) {
        reset_idle_in_step(ai);
        if let Some(poi) = state.poi {
            let at = [f64::from(poi.x), f64::from(poi.y), f64::from(poi.z)];
            navigation::move_to(ai, at, THROUGH_SPEED);
        }
        state.stuck = false;
    }

    fn stop(&self, state: &mut Through, ai: &mut Ai<'_, '_>) {
        if let Some(poi) = state.poi
            && closer_to_centre(poi, ai.position(), DISTANCE_TO_POI)
        {
            state.visited.push(poi);
        }
    }

    fn tick(&self, state: &mut Through, ai: &mut Ai<'_, '_>) {
        if !navigation::is_done(ai) {
            return;
        }
        let Some(poi) = state.poi else {
            return;
        };
        let target = bottom_centre(poi);
        let towards = random_pos::default_pos_towards(ai, 16, 7, target, 0.314_159_274_101_257_3)
            .or_else(|| {
                random_pos::default_pos_towards(ai, 8, 7, target, std::f64::consts::FRAC_PI_2)
            });
        match towards {
            Some(at) => {
                navigation::move_to(ai, at, THROUGH_SPEED);
            }
            None => state.stuck = true,
        }
    }
}

/// `Raider$RaiderCelebration`: a living raider with no target whose raid was lost
/// celebrates (`IS_CELEBRATING`): one tick in `adjustedTickDelay(100)` its celebration's
/// sound, and one in `adjustedTickDelay(50)` a jump where it rides nothing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RaiderCelebration;

impl Goal for RaiderCelebration {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        !ai.tick.mob.body.vitals.dying()
            && ai.tick.mob.body.combat.target.is_none()
            && current_raid(ai.tick).is_some_and(|raid| raid.is_loss())
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        illagers::update(&mut ai.tick.mob, |raider| raider.celebrating = true);
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        illagers::update(&mut ai.tick.mob, |raider| raider.celebrating = false);
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        // No mob is silent in Clustine.
        let sound_odds = self.adjusted_tick_delay(100) as u32;
        if ai.tick.draws.next_int(sound_odds) == 0 {
            let volume = ai.tick.mob_type.sound_volume_of(&ai.tick.mob);
            let sound = illagers::celebrate_sound(ai.tick.kind);
            crate::mobs::body::make_sound(ai.tick, sound, volume);
        }
        if ai.tick.vehicle.is_none() {
            let jump_odds = self.adjusted_tick_delay(50) as u32;
            if ai.tick.draws.next_int(jump_odds) == 0 {
                controls::jump(ai);
            }
        }
    }
}

/// `Vindicator$VindicatorBreakDoorGoal`: of a vindicator in an active raid, one tick in
/// `reducedTickDelay(10)` it asks `BreakDoorGoal.canUse`, which refuses on easy, the
/// world's difficulty (`DOOR_BREAKING_PREDICATE`: normal and hard): it draws, and never
/// starts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VindicatorBreakDoorGoal;

impl Goal for VindicatorBreakDoorGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if !has_active_raid(ai.tick) {
            return false;
        }
        if ai.tick.draws.next_int(reduced_tick_delay(10) as u32) != 0 {
            return false;
        }
        // `BreakDoorGoal.canUse`: its door is asked of (no draw), then the difficulty,
        // which refuses on easy, the world's; the breaking that would follow on normal
        // and hard is not built.
        false
    }
}

/// `DoorInteractGoal`'s and `OpenDoorGoal`'s state: the door (`doorPos`, `hasDoor`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Doorway {
    door: Option<BlockPos>,
}

/// The doors a mob opens by hand (`DoorBlock.isWoodenDoor`: a door whose type opens by
/// hand, the wooden and the copper ones).
fn is_wooden_door(state: clustine_data::BlockState) -> bool {
    clustine_blocks::data::in_tag(state, "minecraft:mob_interactable_doors")
        && state.property("hinge").is_some()
}

/// `AbstractIllager$RaiderOpenDoorGoal` (`OpenDoorGoal(mob, false)`) of a raider in an
/// active raid: where it bumps into something on its path and a door that opens by hand
/// is at one of its path's next nodes within 1.5 (or above its feet), it opens it, and
/// shuts it again as the goal stops, which it does at once (`closeDoor` false).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RaiderOpenDoorGoal;

impl RaiderOpenDoorGoal {
    /// `DoorInteractGoal.canUse`.
    fn door(ai: &Ai<'_, '_>) -> Option<BlockPos> {
        if !navigation::can_navigate_ground(ai) || !ai.tick.living.body.horizontal_collision {
            return None;
        }
        let path = ai.tick.mob.body.path.followed.as_ref()?;
        let next = ai.tick.mob.body.progress.next as usize;
        if next >= path.nodes.len() {
            return None;
        }
        let position = ai.position();
        for node in path.nodes.iter().take((next + 2).min(path.nodes.len())) {
            let door = BlockPos::new(node.x, node.y + 1, node.z);
            let at = [f64::from(door.x), position[1], f64::from(door.z)];
            if physics::distance_sqr(position, at) > 2.25 {
                continue;
            }
            if is_wooden_door(ai.tick.world.state(door)) {
                return Some(door);
            }
        }
        let above = ai.block_position().offset(0, 1, 0);
        is_wooden_door(ai.tick.world.state(above)).then_some(above)
    }

    /// `DoorInteractGoal.setOpen(open)`.
    fn set_open(state: &Doorway, ai: &mut Ai<'_, '_>, open: bool) {
        let Some(door) = state.door else {
            return;
        };
        let block = ai.tick.world.state(door);
        if block.property("hinge").is_some() {
            ai.tick.set_door_open(door, block, open);
        }
    }
}

impl Goal for RaiderOpenDoorGoal {
    type State = Doorway;

    fn flags(&self) -> u8 {
        0
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Doorway, ai: &mut Ai<'_, '_>) -> bool {
        state.door = Self::door(ai);
        state.door.is_some() && has_active_raid(ai.tick)
    }

    fn can_continue(&self, _: &mut Doorway, _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, state: &mut Doorway, ai: &mut Ai<'_, '_>) {
        Self::set_open(state, ai, true);
    }

    fn stop(&self, state: &mut Doorway, ai: &mut Ai<'_, '_>) {
        Self::set_open(state, ai, false);
    }
}

/// `NearestHealableRaiderTargetGoal`'s state: `TargetGoal`'s, and the end of its cooldown
/// (`cooldown`), as the mob's tick.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Healing {
    targeting: Targeting,
    until: u32,
}

/// `NearestHealableRaiderTargetGoal(witch, Raider, true, selector)`: while its cooldown
/// is out, `nextBoolean()`; then, of a witch in an active raid, the nearest raider but a
/// witch it sees within its follow range becomes its target, which it heals with its
/// potions, and its cooldown is `reducedTickDelay(200)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NearestHealableRaiderTargetGoal;

/// The witch's selector: of a witch in an active raid, any raider but a witch.
fn healable(ai: &Ai<'_, '_>, looked: &sensing::Looked) -> bool {
    has_active_raid(ai.tick) && looked.kind != Some(entity_types::WITCH as u16)
}

impl Goal for NearestHealableRaiderTargetGoal {
    type State = Healing;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Healing, ai: &mut Ai<'_, '_>) -> bool {
        if ai.tick.lived < state.until || !ai.tick.draws.next_bool() {
            return false;
        }
        if !has_active_raid(ai.tick) {
            return false;
        }
        let found = find_nearest(ai, Among::Mobs(Class::Kinds(RAIDERS)), &healable);
        state.targeting.found = found.map(|found| found.key);
        state.targeting.found.is_some()
    }

    fn can_continue(&self, state: &mut Healing, ai: &mut Ai<'_, '_>) -> bool {
        TargetRules {
            must_see: true,
            must_reach: false,
        }
        .can_continue(&mut state.targeting, ai)
    }

    fn start(&self, state: &mut Healing, ai: &mut Ai<'_, '_>) {
        state.until = ai.tick.lived + reduced_tick_delay(200) as u32;
        let found = state.targeting.found.and_then(|key| sensing::find(ai, key));
        sensing::set_target(ai, found.as_ref());
        TargetRules::start(&mut state.targeting);
    }

    fn stop(&self, state: &mut Healing, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(&mut state.targeting, ai);
    }
}
