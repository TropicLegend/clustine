//! The villager's points of interest and work (`docs/groundwork/n10-plan.md`, section 6;
//! `n10-brief.md`, section 3): claiming job sites, beds and the bell (`AcquirePoi`),
//! keeping and losing them (`ValidateNearbyPoi`, `YieldJobSite`, `PoiCompetitorScan`),
//! professions (`AssignProfessionFromJobSite`, `ResetProfession`), walking to points
//! (`GoToPotentialJobSite`, `SetWalkTargetFromBlockMemory`, the strolls) and working there
//! (`WorkAtPoi`, the farmer's `WorkAtComposter`, `HarvestFarmland` and `UseBonemeal`).
//!
//! **Points of interest**: what a behaviour reads of them is the region's (`MobWorld`'s
//! queries over its land, copies and the published margin); a ticket it takes or gives
//! back is a local (`Local::TakePoi`, `Local::ReleasePoi`) the region applies after the
//! villager's tick and before any other's, so that a behaviour acts as though it took.
//! Only a record in the region's land has space to take (decision 2).
//!
//! **Other villagers**: what a behaviour reads of a neighbour is its record: its type's
//! state (profession, experience) and, for one the region holds, its brain's memories as
//! its last tick left them; for one only a copy shows, what the copy carries of them
//! (`VillagerState::shown`, decision 3). What it writes into one is `Local::Villager`
//! (`VillagerUse::Yield`, `VillagerUse::LoseJobSite`), which the holder applies, checking
//! a yield's conditions again as it lands (`region/villagers_across.rs`).
//!
//! **Draws**: each is the villager's stream's at the game's place (the level's on its
//! behalf and its own alike, B 7.1.7). Where the game iterates a hash set (the targets of
//! `AcquirePoi`'s path search, its retries) the order here is the targets' nearest first
//! (named): the draws are as many, their values go to other targets.
//!
//! **The brain made again** (`refreshBrain`, at a profession taken or lost): the
//! behaviour asks for it ([`refresh_brain`]), the new brain's sensor delays drawn at that
//! moment; the brain becomes it at the end of its tick (`brain::run`, whose answer tells
//! the villager to take its schedule's activity, as `registerBrainGoals` does).

#![allow(dead_code, reason = "part V1 names these in the villager's packages")]

use std::collections::BTreeMap;

use clustine_blocks::data::{flag, in_tag, item_in_tag};
use clustine_data::villagers::professions;
use clustine_data::{BlockClass, ITEMS, entity_types, items, sounds};
use clustine_world::poi::PoiType;
use clustine_world::{BlockPos, EntityData, EntityId, Entry, ItemStack, VillagerUse};
use serde::{Deserialize, Serialize};

use super::super::sensors::find;
use super::super::{
    Activity, Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Slot, Status, Timed, Tracker,
    Value, ids,
};
use super::walking::HALF_PI;
use super::{HeldKept, KeptShot, bottom_centre, centre, manhattan, walk, walk_to_point};
use crate::living::slots;
use crate::mobs::ai::{navigation, random_pos};
use crate::mobs::body::{self, Event};
use crate::mobs::tick::Local;
use crate::mobs::types::villager;
use crate::physics::{self, Vec3, World};
use crate::region::composting;
use crate::region::pois::{Occupancy, PoiRecord};

// ------------------------------------------------------------------------------------
// What the behaviours share.

/// The profession [`validate_job_site`] reads as the villager's at each tick rather than
/// one fixed as its brain was made: the same, since the brain is made again at every
/// change of profession (`AssignProfessionFromJobSite`, `ResetProfession`, the cure), for
/// a provider that cannot pass the profession a brain is made with.
pub(crate) const PROFESSION_NOW: u8 = u8::MAX;

/// `GlobalPos` memories hold a block of the one dimension: the block a memory names.
fn block_memory(cx: &Cx<'_, '_, '_>, kind: MemoryKind) -> Option<BlockPos> {
    match cx.memories.get(kind)? {
        Value::Block(pos) => Some(pos),
        _ => None,
    }
}

/// `BlockPos.closerToCenterThan(position, distance)`: the block's centre strictly within
/// `distance` of the point.
pub(crate) fn closer_to_centre(pos: BlockPos, position: Vec3, distance: f64) -> bool {
    let at = centre(pos);
    let d = physics::sub(at, position);
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < distance * distance
}

/// `BehaviorUtils.setWalkAndLookTargetMemories(mob, pos, speed, close)`: it looks at the
/// block and walks to it.
fn set_walk_and_look(cx: &mut Cx<'_, '_, '_>, pos: BlockPos, speed: f32, close: i32) {
    cx.memories
        .set(MemoryKind::LookTarget, Value::Tracker(Tracker::Block(pos)));
    cx.memories.set(
        MemoryKind::WalkTarget,
        walk(Tracker::Block(pos), speed, close),
    );
}

/// The villager's profession, by its registry id.
fn profession(cx: &Cx<'_, '_, '_>) -> i32 {
    i32::from(villager::state_of(&cx.ai.tick.mob).1.data.profession)
}

/// `isBaby()`.
fn is_baby(cx: &Cx<'_, '_, '_>) -> bool {
    cx.ai.tick.mob_type.is_baby(&cx.ai.tick.mob)
}

/// The type of the point of interest at `pos` (`PoiManager.getType`).
fn poi_type_at(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> Option<PoiType> {
    cx.ai.tick.world.poi_type_at(pos)
}

/// `Villager.POI_MEMORIES`: which types the memory `kind` holds for a villager of
/// `profession`.
fn memory_holds(kind: MemoryKind, profession: i32, poi: PoiType) -> bool {
    match kind {
        MemoryKind::Home => poi == PoiType::Home,
        MemoryKind::JobSite => PoiType::held_by(profession) == Some(poi),
        MemoryKind::PotentialJobSite => poi.is_job_site(),
        MemoryKind::MeetingPoint => poi == PoiType::Meeting,
        _ => false,
    }
}

/// `Villager.releasePoi(memory)`: the ticket of the point the memory names given back
/// where a point of a type the memory holds is there (`PoiManager.release`); the memory
/// itself is kept.
pub(crate) fn release_memory_poi(cx: &mut Cx<'_, '_, '_>, kind: MemoryKind) {
    let Some(pos) = block_memory(cx, kind) else {
        return;
    };
    let Some(poi) = poi_type_at(cx, pos) else {
        return;
    };
    if memory_holds(kind, profession(cx), poi) {
        cx.ai
            .tick
            .out
            .local
            .push(Local::ReleasePoi { pos, kind: poi });
    }
}

/// `refreshBrain`: the brain made again at the end of this tick, its sensors' delays
/// drawn now from the villager's own stream, as `makeBrain` draws them
/// (`docs/groundwork/n10-brief.md`, finding 10). The memories the game saves go on.
pub(crate) fn refresh_brain(cx: &mut Cx<'_, '_, '_>) {
    let Some(provider) = cx.ai.tick.mob_type.brain() else {
        return;
    };
    let first = cx.ai.tick.lived.saturating_add(1);
    let state = crate::mobs::brain::made(provider, first, cx.ai.tick.draws);
    cx.memories.refresh = Some(state);
}

/// `Villager.setVillagerData` with a new profession: its offers go where the profession
/// changes.
fn set_profession(cx: &mut Cx<'_, '_, '_>, profession: i32) {
    let profession = u8::try_from(profession).expect("a profession's registry id fits a byte");
    villager::update(&mut cx.ai.tick.mob, |own| {
        if own.data.profession != profession {
            own.offers = None;
        }
        own.data.profession = profession;
    });
}

/// A memory of another mob as its record keeps it: its brain's slot among its mind's
/// parts, none where it has none (an expiry is not looked at: the points of interest
/// never expire).
fn recorded_memory(mind: &[Entry], kind: MemoryKind) -> Option<Value> {
    let id = ids::FIRST_MEMORY + kind.id();
    let entry = mind.iter().find(|entry| entry.id == id)?;
    match postcard::from_bytes::<Slot>(&entry.bytes).ok()? {
        Slot::Kept { value, .. } => Some(value),
        Slot::Count(_) => None,
    }
}

/// A villager's record as these behaviours read it, without decoding the whole mob: its
/// mind's parts and its own state.
fn villager_record(entity: &clustine_world::Entity) -> Option<(&[Entry], villager::VillagerState)> {
    let EntityData::Mob { mind, state, .. } = &entity.data else {
        return None;
    };
    let (_, own) = crate::mobs::types::animal::read::<villager::VillagerState>(state);
    Some((mind, own))
}

/// A villager near this one, as a behaviour reads it.
struct Neighbour {
    key: EntityId,
    profession: i32,
    xp: i32,
    job_site: Option<BlockPos>,
    potential_job_site: Option<BlockPos>,
    position: Vec3,
    on_ground: bool,
}

/// The living villagers other than this one in `nearest_living_entities` (`mobs`),
/// nearest first, as their records show them: the region's own with their brains'
/// memories, one in a copy with what the copy carries of them.
fn neighbours(cx: &Cx<'_, '_, '_>) -> Vec<Neighbour> {
    let Some(Value::Entities(keys)) = cx.memories.get(MemoryKind::Mobs) else {
        return Vec::new();
    };
    let me = cx.ai.tick.key;
    let villager_kind = entity_types::VILLAGER as u16;
    keys.into_iter()
        .filter(|key| *key != me)
        .filter_map(|key| {
            let looked = find(cx.ai, key)?;
            if looked.kind != Some(villager_kind) || !looked.alive {
                return None;
            }
            let seen = cx.ai.tick.world.entity(key)?;
            let (mind, own) = villager_record(seen.entity)?;
            let block = |value: Option<Value>| match value {
                Some(Value::Block(pos)) => Some(pos),
                _ => None,
            };
            let (job_site, potential_job_site) = if seen.copied {
                (own.shown.job_site, own.shown.potential_job_site)
            } else {
                (
                    block(recorded_memory(mind, MemoryKind::JobSite)),
                    block(recorded_memory(mind, MemoryKind::PotentialJobSite)),
                )
            };
            Some(Neighbour {
                key,
                profession: i32::from(own.data.profession),
                xp: own.xp,
                job_site,
                potential_job_site,
                position: seen.entity.position,
                on_ground: seen.entity.on_ground,
            })
        })
        .collect()
}

// ------------------------------------------------------------------------------------
// `AcquirePoi` (AcquirePoi.txt; the brief's 3.1, finding 12).

/// How far `AcquirePoi` looks (`SCAN_RANGE`).
const SCAN_RANGE: i32 = 48;
/// How many of the nearest it asks a path to.
const SCAN_TARGETS: usize = 5;

/// `AcquirePoi$JitteredLinearRetry`: a target a path did not reach, tried again later
/// and later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Retry {
    pub(crate) previous: i64,
    pub(crate) next: i64,
    pub(crate) delay: i32,
}

impl Retry {
    /// Its constructor at the game time `time`: a first attempt marked.
    pub(crate) fn new(draws: &mut crate::region::Random, time: i64) -> Self {
        let mut retry = Self {
            previous: 0,
            next: 0,
            delay: 0,
        };
        retry.mark(draws, time);
        retry
    }

    /// `markAttempt(time)`: the delay grows by 40 to 79 ticks (one `nextInt(40)`), to at
    /// most 400, from now.
    pub(crate) fn mark(&mut self, draws: &mut crate::region::Random, time: i64) {
        self.previous = time;
        let grown = self.delay + draws.next_int(40) as i32 + 40;
        self.delay = grown.min(400);
        self.next = time + i64::from(self.delay);
    }

    /// `isStillValid(time)`: tried within the last 400 ticks.
    pub(crate) fn still_valid(&self, time: i64) -> bool {
        time - self.previous < 400
    }

    /// `shouldRetry(time)`.
    pub(crate) fn should_retry(&self, time: i64) -> bool {
        time >= self.next
    }
}

/// What an `AcquirePoi` keeps: its next scan (`MutableLong`, 0 before its first call) and
/// its retries by target (`batchCache`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Acquiring {
    pub(crate) next: i64,
    pub(crate) retries: BTreeMap<BlockPos, Retry>,
}

/// `AcquirePoi.create(type, memoryToValidate, memoryToAcquire, onlyIfAdult, entityEvent,
/// validPoi)`: with neither memory set, it scans every 20 to 39 ticks (the first after
/// up to 19) for the nearest points of the type with a free ticket within 48, those on
/// a retry's wait left out, makes one path to up to five of them, and where it reaches
/// one takes its ticket and remembers it; else each is put on a retry.
pub(crate) struct AcquirePoi {
    /// The types it takes, of the villager's profession by its registry id.
    pub(crate) wanted: fn(i32, PoiType) -> bool,
    pub(crate) acquire: MemoryKind,
    /// The memory that must be absent beside the one acquired (the job site, for the
    /// potential job site), if another.
    pub(crate) validate: Option<MemoryKind>,
    pub(crate) only_adult: bool,
    pub(crate) event: Option<u8>,
    /// `validPoi`, of the level and the point's block.
    pub(crate) valid: fn(&Cx<'_, '_, '_>, BlockPos) -> bool,
    pub(crate) required: Vec<MemoryKind>,
}

impl AcquirePoi {
    fn new(
        wanted: fn(i32, PoiType) -> bool,
        acquire: MemoryKind,
        validate: Option<MemoryKind>,
        only_adult: bool,
        event: Option<u8>,
        valid: fn(&Cx<'_, '_, '_>, BlockPos) -> bool,
    ) -> Self {
        let required = std::iter::once(acquire).chain(validate).collect();
        Self {
            wanted,
            acquire,
            validate,
            only_adult,
            event,
            valid,
            required,
        }
    }
}

/// `lambda$getCorePackage$0`: every block passes.
fn any_block(_: &Cx<'_, '_, '_>, _: BlockPos) -> bool {
    true
}

/// `Villager.validateBedPoi(level, pos)`: a bed villagers sleep on, not occupied.
fn bed_free(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    let state = cx.ai.tick.world.state(pos);
    in_tag(state, "minecraft:villagers_can_sleep_on_bed") && !flag(state, "occupied")
}

impl KeptShot for AcquirePoi {
    type State = Acquiring;

    fn required(&self) -> &[MemoryKind] {
        &self.required
    }

    fn trigger(&self, state: &mut Acquiring, cx: &mut Cx<'_, '_, '_>) -> bool {
        // The outer builder's `absent(memoryToValidate)`, then the inner's.
        if let Some(validate) = self.validate
            && !cx.memories.check(validate, Status::Absent)
        {
            return false;
        }
        if !cx.memories.check(self.acquire, Status::Absent) {
            return false;
        }
        if self.only_adult && is_baby(cx) {
            return false;
        }
        let time = cx.time;
        if state.next == 0 {
            state.next = time + i64::from(cx.ai.tick.draws.next_int(20));
            return false;
        }
        if time < state.next {
            return false;
        }
        state.next = time + 20 + i64::from(cx.ai.tick.draws.next_int(20));
        state.retries.retain(|_, retry| retry.still_valid(time));
        let at = cx.ai.block_position();
        let wanted = self.wanted;
        let of = profession(cx);
        let found = cx.ai.tick.world.pois_in_range(
            at,
            SCAN_RANGE,
            &|kind| wanted(of, kind),
            Occupancy::HasSpace,
        );
        // The retries' test runs on every match in the query's order, before the sort
        // (a barrier): a due retry draws as it is marked.
        let mut passing: Vec<PoiRecord> = Vec::new();
        for record in found {
            let passes = match state.retries.get_mut(&record.pos) {
                None => true,
                Some(retry) if !retry.should_retry(time) => false,
                Some(retry) => {
                    retry.mark(cx.ai.tick.draws, time);
                    true
                }
            };
            if passes {
                passing.push(record);
            }
        }
        passing.sort_by(|a, b| a.dist_sqr(at).total_cmp(&b.dist_sqr(at)));
        passing.truncate(SCAN_TARGETS);
        passing.retain(|record| (self.valid)(cx, record.pos));
        let reach = passing
            .iter()
            .map(|record| i32::from(record.kind.valid_range()))
            .fold(1, i32::max);
        let targets: Vec<BlockPos> = passing.iter().map(|record| record.pos).collect();
        let reached = if targets.is_empty() {
            None
        } else {
            navigation::create_path_to_any(cx.ai, &targets, reach).and_then(|made| {
                made.path(cx.ai)
                    .filter(|path| path.reached)
                    .map(|path| path.target)
            })
        };
        match reached {
            Some(target) => {
                if poi_type_at(cx, target).is_some() {
                    // `take(type, pos == target, target, 1)`, its answer ignored.
                    cx.ai.tick.out.local.push(Local::TakePoi { pos: target });
                    cx.memories.set(self.acquire, Value::Block(target));
                    if let Some(event) = self.event {
                        cx.ai.tick.out.events.push(Event::Entity(event));
                    }
                    state.retries.clear();
                }
            }
            None => {
                // `computeIfAbsent`: a new target's retry draws; one waiting already not.
                for pos in targets {
                    state
                        .retries
                        .entry(pos)
                        .or_insert_with(|| Retry::new(cx.ai.tick.draws, time));
                }
            }
        }
        true
    }
}

/// `AcquirePoi.create(acquirableJobSite, job_site, potential_job_site, true, empty)`
/// (core 6): an adult without a job site reserves one.
pub(crate) fn acquire_job_site() -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(AcquirePoi::new(
        |profession, kind| kind.acquirable_by(profession),
        MemoryKind::PotentialJobSite,
        Some(MemoryKind::JobSite),
        true,
        None,
        any_block,
    )))
}

/// `AcquirePoi.create(HOME, home, false, Optional.of(14), validateBedPoi)` (core 10).
pub(crate) fn acquire_home() -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(AcquirePoi::new(
        |_, kind| kind == PoiType::Home,
        MemoryKind::Home,
        None,
        false,
        Some(14),
        bed_free,
    )))
}

/// `AcquirePoi.create(MEETING, meeting_point, true, Optional.of(14))` (core 10).
pub(crate) fn acquire_meeting_point() -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(AcquirePoi::new(
        |_, kind| kind == PoiType::Meeting,
        MemoryKind::MeetingPoint,
        None,
        true,
        Some(14),
        any_block,
    )))
}

// ------------------------------------------------------------------------------------
// Keeping and losing a point (the brief's 3.2).

/// Which types a [`ValidateNearbyPoi`] holds valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holds {
    /// `profession.heldJobSite`, of the profession by its registry id, or the villager's
    /// as it is now ([`PROFESSION_NOW`]).
    JobOf(u8),
    /// `acquirableJobSite`, of the villager's profession as it is now.
    Acquirable,
    Home,
    Meeting,
}

impl Holds {
    fn accepts(self, cx: &Cx<'_, '_, '_>, kind: PoiType) -> bool {
        match self {
            Holds::JobOf(PROFESSION_NOW) => PoiType::held_by(profession(cx)) == Some(kind),
            Holds::JobOf(profession) => PoiType::held_by(i32::from(profession)) == Some(kind),
            Holds::Acquirable => kind.acquirable_by(profession(cx)),
            Holds::Home => kind == PoiType::Home,
            Holds::Meeting => kind == PoiType::Meeting,
        }
    }
}

/// `ValidateNearbyPoi.create(predicate, memory)` (ValidateNearbyPoi.txt 62–146): a point
/// remembered within 16 blocks that is gone or of another type is forgotten; a bed in it
/// that another occupies is forgotten too, and its ticket given back unless a villager
/// sleeps in it.
struct ValidateNearbyPoi {
    holds: Holds,
    memory: MemoryKind,
}

/// [`ValidateNearbyPoi`], boxed.
fn validate(holds: Holds, memory: MemoryKind) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(ValidateNearbyPoi { holds, memory }))
}

/// Whether a villager that sleeps is in the block's box at `pos`
/// (`bedIsOccupiedByVillager`), by what the villagers' records show of their sleep.
fn villager_sleeps_at(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    let bounds = crate::physics::aabb::Aabb {
        min: [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)],
        max: [
            f64::from(pos.x) + 1.0,
            f64::from(pos.y) + 1.0,
            f64::from(pos.z) + 1.0,
        ],
    };
    let villager_kind = entity_types::VILLAGER as u16;
    cx.ai
        .tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| seen.entity.kind == villager_kind)
        .filter_map(|seen| villager_record(seen.entity))
        .any(|(_, own)| own.shown.sleeping)
}

impl Shot for ValidateNearbyPoi {
    fn required(&self) -> &[MemoryKind] {
        std::slice::from_ref(&self.memory)
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(pos) = block_memory(cx, self.memory) else {
            return false;
        };
        if !closer_to_centre(pos, cx.ai.position(), 16.0) {
            return false;
        }
        match poi_type_at(cx, pos).filter(|kind| self.holds.accepts(cx, *kind)) {
            None => cx.memories.erase(self.memory),
            Some(kind) => {
                // `bedIsOccupied`: of the tag, occupied, and this villager not asleep
                // (as its record showed it after its last tick).
                let state = cx.ai.tick.world.state(pos);
                let asleep = villager::state_of(&cx.ai.tick.mob).1.shown.sleeping;
                if in_tag(state, "minecraft:villagers_can_sleep_on_bed")
                    && flag(state, "occupied")
                    && !asleep
                {
                    cx.memories.erase(self.memory);
                    if !villager_sleeps_at(cx, pos) {
                        cx.ai.tick.out.local.push(Local::ReleasePoi { pos, kind });
                    }
                }
            }
        }
        true
    }
}

/// `ValidateNearbyPoi.create(profession.heldJobSite, job_site)` (core 0), of the villager's
/// profession by its registry id, as the brain was made; [`PROFESSION_NOW`] reads it at
/// each tick.
pub(crate) fn validate_job_site(profession: u8) -> Box<dyn Behaviour> {
    validate(Holds::JobOf(profession), MemoryKind::JobSite)
}

/// `ValidateNearbyPoi.create(acquirableJobSite, potential_job_site)` (core 0).
pub(crate) fn validate_potential_job_site() -> Box<dyn Behaviour> {
    validate(Holds::Acquirable, MemoryKind::PotentialJobSite)
}

/// `ValidateNearbyPoi.create(HOME, home)` (rest 3).
pub(crate) fn validate_home() -> Box<dyn Behaviour> {
    validate(Holds::Home, MemoryKind::Home)
}

/// `ValidateNearbyPoi.create(MEETING, meeting_point)` (meet 3).
pub(crate) fn validate_meeting_point() -> Box<dyn Behaviour> {
    validate(Holds::Meeting, MemoryKind::MeetingPoint)
}

/// `PoiCompetitorScan.create()` (PoiCompetitorScan.txt 9–169): with a job site, the
/// nearby living villagers whose job site is the same and whose profession holds its
/// type, each against the winner so far (this one first): the one with more experience
/// wins, the later on a tie; each loser's job site is forgotten, no ticket given back.
struct PoiCompetitorScan;

impl Shot for PoiCompetitorScan {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::JobSite, MemoryKind::Mobs]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::Mobs, Status::Present) {
            return false;
        }
        let Some(site) = block_memory(cx, MemoryKind::JobSite) else {
            return false;
        };
        let Some(kind) = poi_type_at(cx, site) else {
            return true;
        };
        // `None` is this villager, as the winner so far.
        let mut winner: (Option<EntityId>, i32) = (None, villager::state_of(&cx.ai.tick.mob).1.xp);
        for other in neighbours(cx) {
            if other.job_site != Some(site) || PoiType::held_by(other.profession) != Some(kind) {
                continue;
            }
            // `selectWinner(a, b)`: `a` wins only with more experience.
            let (loser, kept) = if winner.1 > other.xp {
                (Some(other.key), winner)
            } else {
                (winner.0, (Some(other.key), other.xp))
            };
            match loser {
                None => cx.memories.erase(MemoryKind::JobSite),
                Some(key) => cx.ai.tick.out.local.push(Local::Villager {
                    key,
                    used: VillagerUse::LoseJobSite { site },
                }),
            }
            winner = kept;
        }
        true
    }
}

/// `PoiCompetitorScan.create()` (core 2).
pub(crate) fn poi_competitor_scan() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(PoiCompetitorScan))
}

/// `GoToPotentialJobSite(speed)` (Behavior 1200/1200): with no active activity but idle,
/// work or play, a walk to the reserved site every tick; at its stop, where the
/// reservation is still there (its 1200 ticks out), the ticket is given back and the
/// reservation forgotten.
struct GoToPotentialJobSite {
    speed: f32,
}

impl Timed for GoToPotentialJobSite {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[(MemoryKind::PotentialJobSite, Status::Present)]
    }

    fn durations(&self) -> (i64, i64) {
        (1200, 1200)
    }

    fn extra_start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories
            .active
            .iter()
            .find(|activity| **activity != Activity::Core)
            .is_none_or(|activity| {
                matches!(activity, Activity::Idle | Activity::Work | Activity::Play)
            })
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories
            .check(MemoryKind::PotentialJobSite, Status::Present)
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if let Some(site) = block_memory(cx, MemoryKind::PotentialJobSite) {
            set_walk_and_look(cx, site, self.speed, 1);
        }
    }

    fn stop(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if let Some(site) = block_memory(cx, MemoryKind::PotentialJobSite)
            && let Some(kind) = poi_type_at(cx, site)
        {
            cx.ai
                .tick
                .out
                .local
                .push(Local::ReleasePoi { pos: site, kind });
        }
        cx.memories.erase(MemoryKind::PotentialJobSite);
    }
}

/// `GoToPotentialJobSite(speed)` (core 7).
pub(crate) fn go_to_potential_job_site(speed: f32) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(GoToPotentialJobSite { speed }))
}

/// `YieldJobSite.create(speed)` (YieldJobSite.txt 120–211): an unemployed adult with a
/// reserved site and no job site gives it to the first nearby living villager with no
/// reservation whose profession holds its type and that either has no job site and can
/// reach the site (a path search as its navigation would make it) or has this one; it
/// forgets its walk, its look and its reservation, and the other (if it has no job site)
/// takes the site as its reservation and walks to it. The ticket is not given back: it
/// passes.
struct YieldJobSite {
    speed: f32,
}

impl Shot for YieldJobSite {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::PotentialJobSite,
            MemoryKind::JobSite,
            MemoryKind::Mobs,
            MemoryKind::WalkTarget,
            MemoryKind::LookTarget,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::JobSite, Status::Absent)
            || !cx.memories.check(MemoryKind::Mobs, Status::Present)
        {
            return false;
        }
        let Some(site) = block_memory(cx, MemoryKind::PotentialJobSite) else {
            return false;
        };
        if is_baby(cx) || profession(cx) != professions::NONE {
            return false;
        }
        let Some(kind) = poi_type_at(cx, site) else {
            return true;
        };
        let reach = i32::from(kind.valid_range());
        // `findFirst` of a lazy stream: the path searches stop at the first that wants it.
        let mut taker = None;
        for other in neighbours(cx) {
            let wants = other.potential_job_site.is_none()
                && PoiType::held_by(other.profession) == Some(kind)
                && match other.job_site {
                    None => navigation::other_can_reach(
                        cx.ai,
                        other.position,
                        other.on_ground,
                        false,
                        site,
                        reach,
                    ),
                    Some(job_site) => job_site == site,
                };
            if wants {
                taker = Some(other);
                break;
            }
        }
        if let Some(other) = taker {
            cx.memories.erase(MemoryKind::WalkTarget);
            cx.memories.erase(MemoryKind::LookTarget);
            cx.memories.erase(MemoryKind::PotentialJobSite);
            if other.job_site.is_none() {
                cx.ai.tick.out.local.push(Local::Villager {
                    key: other.key,
                    used: VillagerUse::Yield {
                        site,
                        kind,
                        speed: clustine_world::Float(self.speed),
                    },
                });
            }
        }
        true
    }
}

/// `YieldJobSite.create(speed)` (core 8).
pub(crate) fn yield_job_site(speed: f32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(YieldJobSite { speed }))
}

// ------------------------------------------------------------------------------------
// Professions (finding 13; the brief's 3.5).

/// `AssignProfessionFromJobSite.create()` (39–133): within 2 of its reserved site's
/// centre, the reservation becomes its job site and it shows its green sparks (entity
/// event 14); an unemployed one takes the first profession in registry order that holds
/// the site's type, and its brain is made again.
struct AssignProfessionFromJobSite;

impl Shot for AssignProfessionFromJobSite {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::PotentialJobSite, MemoryKind::JobSite]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(site) = block_memory(cx, MemoryKind::PotentialJobSite) else {
            return false;
        };
        if !closer_to_centre(site, cx.ai.position(), 2.0) {
            return false;
        }
        cx.memories.erase(MemoryKind::PotentialJobSite);
        cx.memories.set(MemoryKind::JobSite, Value::Block(site));
        cx.ai.tick.out.events.push(Event::Entity(14));
        if profession(cx) != professions::NONE {
            return true;
        }
        if let Some(kind) = poi_type_at(cx, site)
            && let Some(taken) = (0..=professions::WEAPONSMITH)
                .find(|profession| PoiType::held_by(*profession) == Some(kind))
        {
            set_profession(cx, taken);
            refresh_brain(cx);
        }
        true
    }
}

/// `AssignProfessionFromJobSite.create()` (core 10).
pub(crate) fn assign_profession_from_job_site() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(AssignProfessionFromJobSite))
}

/// `ResetProfession.create()` (32–75): with no job site, a villager of a profession (not
/// a nitwit) with no experience and at level 1 becomes unemployed, its brain made again.
struct ResetProfession;

impl Shot for ResetProfession {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::JobSite]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::JobSite, Status::Absent) {
            return false;
        }
        let (_, own) = villager::state_of(&cx.ai.tick.mob);
        let working = !matches!(
            i32::from(own.data.profession),
            professions::NONE | professions::NITWIT
        );
        if working && own.xp == 0 && own.data.level <= 1 {
            set_profession(cx, professions::NONE);
            refresh_brain(cx);
            return true;
        }
        false
    }
}

/// `ResetProfession.create()` (core 10).
pub(crate) fn reset_profession() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(ResetProfession))
}

// ------------------------------------------------------------------------------------
// Walking to points (the brief's 1.5).

/// `SetWalkTargetFromBlockMemory.create(memory, speed, close, tooFar, tooLong)`
/// (SetWalkTargetFromBlockMemory.txt): with no walk target, to the point remembered: one
/// unreachable for more than `too_long` ticks is given back and forgotten; one farther
/// than `too_far` (Manhattan) is walked towards, by a place towards it within 15 and 7
/// that is near enough, up to 1000 tries, after which it is given back and forgotten;
/// one farther than `close` is walked to.
struct SetWalkTargetFromBlockMemory {
    memory: MemoryKind,
    speed: f32,
    close: i32,
    too_far: i32,
    too_long: i32,
    required: [MemoryKind; 3],
}

impl SetWalkTargetFromBlockMemory {
    /// `releasePoi`, the memory forgotten, and `cant_reach_walk_target_since` now.
    fn give_up(&self, cx: &mut Cx<'_, '_, '_>) {
        release_memory_poi(cx, self.memory);
        cx.memories.erase(self.memory);
        cx.memories
            .set(MemoryKind::CantReachWalkTargetSince, Value::Long(cx.time));
    }
}

impl Shot for SetWalkTargetFromBlockMemory {
    fn required(&self) -> &[MemoryKind] {
        &self.required
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent) {
            return false;
        }
        let Some(pos) = block_memory(cx, self.memory) else {
            return false;
        };
        let since = match cx.memories.get(MemoryKind::CantReachWalkTargetSince) {
            Some(Value::Long(since)) => Some(since),
            _ => None,
        };
        if since.is_some_and(|since| cx.time - since > i64::from(self.too_long)) {
            self.give_up(cx);
            return true;
        }
        let here = cx.ai.block_position();
        if manhattan(pos, here) > self.too_far {
            let mut found: Option<Vec3> = None;
            let mut tries = 0;
            loop {
                if found.is_some_and(|at| {
                    manhattan(physics::containing(at), cx.ai.block_position()) <= self.too_far
                }) {
                    break;
                }
                found = random_pos::default_pos_towards(cx.ai, 15, 7, bottom_centre(pos), HALF_PI);
                tries += 1;
                // The game's loop gives up at its thousandth try, found or not.
                if tries == 1000 {
                    self.give_up(cx);
                    return true;
                }
            }
            if let Some(at) = found {
                cx.memories.set(
                    MemoryKind::WalkTarget,
                    walk_to_point(at, self.speed, self.close),
                );
            }
        } else if manhattan(pos, here) > self.close {
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk(Tracker::Block(pos), self.speed, self.close),
            );
        }
        true
    }
}

/// `SetWalkTargetFromBlockMemory.create(memory, speed, close, tooFar, tooLong)`.
pub(crate) fn set_walk_target_from_block_memory(
    memory: MemoryKind,
    speed: f32,
    close: i32,
    too_far: i32,
    too_long: i32,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(SetWalkTargetFromBlockMemory {
        memory,
        speed,
        close,
        too_far,
        too_long,
        required: [
            MemoryKind::CantReachWalkTargetSince,
            MemoryKind::WalkTarget,
            memory,
        ],
    }))
}

/// The three strolls about a point (`StrollAroundPoi`, `StrollToPoi`, `StrollToPoiList`):
/// each fires while its point is within `max_distance` of the villager, and sets a walk
/// target only once its own wait (`MutableLong`) is past.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Stroll {
    /// `StrollAroundPoi`: a random place on land within 8 and 6 (none erases the walk
    /// target), every 180 ticks.
    Around,
    /// `StrollToPoi`: to the point, close within `close`, every 80 ticks.
    To { close: i32 },
    /// `StrollToPoiList`: to one of the list `list` (drawn `nextInt(size)` before its
    /// wait is looked at), close within `close`, every 100 ticks.
    ToList { list: MemoryKind, close: i32 },
}

struct StrollAboutPoi {
    memory: MemoryKind,
    speed: f32,
    max_distance: i32,
    stroll: Stroll,
    required: Vec<MemoryKind>,
}

impl KeptShot for StrollAboutPoi {
    type State = i64;

    fn required(&self) -> &[MemoryKind] {
        &self.required
    }

    fn trigger(&self, next: &mut i64, cx: &mut Cx<'_, '_, '_>) -> bool {
        let time = cx.time;
        let Some(point) = block_memory(cx, self.memory) else {
            return false;
        };
        if let Stroll::ToList { list, close } = self.stroll {
            let Some(Value::Blocks(points)) = cx.memories.get(list) else {
                return false;
            };
            if points.is_empty() {
                return false;
            }
            let picked = points[cx.ai.tick.draws.next_int(points.len() as u32) as usize];
            if !closer_to_centre(point, cx.ai.position(), f64::from(self.max_distance)) {
                return false;
            }
            if time > *next {
                cx.memories.set(
                    MemoryKind::WalkTarget,
                    walk(Tracker::Block(picked), self.speed, close),
                );
                *next = time + 100;
            }
            return true;
        }
        if !closer_to_centre(point, cx.ai.position(), f64::from(self.max_distance)) {
            return false;
        }
        if time <= *next {
            return true;
        }
        match self.stroll {
            Stroll::Around => {
                match random_pos::land_pos(cx.ai, 8, 6) {
                    Some(at) => cx
                        .memories
                        .set(MemoryKind::WalkTarget, walk_to_point(at, self.speed, 1)),
                    None => cx.memories.erase(MemoryKind::WalkTarget),
                }
                *next = time + 180;
            }
            Stroll::To { close } => {
                cx.memories.set(
                    MemoryKind::WalkTarget,
                    walk(Tracker::Block(point), self.speed, close),
                );
                *next = time + 80;
            }
            Stroll::ToList { .. } => {}
        }
        true
    }
}

/// `StrollAroundPoi.create(memory, speed, maxDistance)`.
pub(crate) fn stroll_around_poi(
    memory: MemoryKind,
    speed: f32,
    max_distance: i32,
) -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(StrollAboutPoi {
        memory,
        speed,
        max_distance,
        stroll: Stroll::Around,
        required: vec![MemoryKind::WalkTarget, memory],
    }))
}

/// `StrollToPoi.create(memory, speed, close, maxDistance)`.
pub(crate) fn stroll_to_poi(
    memory: MemoryKind,
    speed: f32,
    close: i32,
    max_distance: i32,
) -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(StrollAboutPoi {
        memory,
        speed,
        max_distance,
        stroll: Stroll::To { close },
        required: vec![MemoryKind::WalkTarget, memory],
    }))
}

/// `StrollToPoiList.create(list, speed, close, maxDistance, memory)`.
pub(crate) fn stroll_to_poi_list(
    list: MemoryKind,
    speed: f32,
    close: i32,
    max_distance: i32,
    memory: MemoryKind,
) -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(StrollAboutPoi {
        memory,
        speed,
        max_distance,
        stroll: Stroll::ToList { list, close },
        required: vec![MemoryKind::WalkTarget, list, memory],
    }))
}

// ------------------------------------------------------------------------------------
// Work (the brief's 3.3).

/// How far from its job site's centre a villager works (`WorkAtPoi.DISTANCE`).
const WORK_DISTANCE: f64 = 1.73;
/// The ticks between two tries of `WorkAtPoi` (`CHECK_COOLDOWN`).
const CHECK_COOLDOWN: i64 = 300;

/// `VillagerProfession.workSound()` of a profession, by its registry id: none for none
/// and the nitwit.
fn work_sound(profession: i32) -> Option<i32> {
    Some(match profession {
        professions::ARMORER => sounds::ENTITY_VILLAGER_WORK_ARMORER,
        professions::BUTCHER => sounds::ENTITY_VILLAGER_WORK_BUTCHER,
        professions::CARTOGRAPHER => sounds::ENTITY_VILLAGER_WORK_CARTOGRAPHER,
        professions::CLERIC => sounds::ENTITY_VILLAGER_WORK_CLERIC,
        professions::FARMER => sounds::ENTITY_VILLAGER_WORK_FARMER,
        professions::FISHERMAN => sounds::ENTITY_VILLAGER_WORK_FISHERMAN,
        professions::FLETCHER => sounds::ENTITY_VILLAGER_WORK_FLETCHER,
        professions::LEATHERWORKER => sounds::ENTITY_VILLAGER_WORK_LEATHERWORKER,
        professions::LIBRARIAN => sounds::ENTITY_VILLAGER_WORK_LIBRARIAN,
        professions::MASON => sounds::ENTITY_VILLAGER_WORK_MASON,
        professions::SHEPHERD => sounds::ENTITY_VILLAGER_WORK_SHEPHERD,
        professions::TOOLSMITH => sounds::ENTITY_VILLAGER_WORK_TOOLSMITH,
        professions::WEAPONSMITH => sounds::ENTITY_VILLAGER_WORK_WEAPONSMITH,
        _ => return None,
    })
}

/// `Villager.shouldRestock` and `restock` at `WorkAtPoi.start`: trading's
/// (`trading/merchant.rs`); it draws nothing.
pub(crate) fn restock_if_due(cx: &mut Cx<'_, '_, '_>) {
    let day_ticks = cx.ai.tick.world.day();
    crate::trading::merchant::restock_if_due(&mut cx.ai.tick.mob, cx.time, day_ticks);
}

/// `WorkAtPoi` (Behavior 60/60; WorkAtPoi.txt 20–133) and the farmer's `WorkAtComposter`,
/// which differs in its `useWorkstation`: 300 ticks after its last check, half the time
/// (one `nextInt(2)` each try), within 1.73 of its job site's centre; at its start the
/// time remembered, a look at the site, the profession's work sound, the station used,
/// and a restock where one is due.
struct WorkAtPoi {
    composter: bool,
}

impl Timed for WorkAtPoi {
    /// `lastCheck`.
    type State = i64;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::JobSite, Status::Present),
            (MemoryKind::LookTarget, Status::Registered),
        ]
    }

    fn extra_start(&self, last_check: &mut i64, cx: &mut Cx<'_, '_, '_>) -> bool {
        if cx.time - *last_check < CHECK_COOLDOWN {
            return false;
        }
        if cx.ai.tick.draws.next_int(2) != 0 {
            return false;
        }
        *last_check = cx.time;
        block_memory(cx, MemoryKind::JobSite)
            .is_some_and(|site| closer_to_centre(site, cx.ai.position(), WORK_DISTANCE))
    }

    fn can_still_use(&self, _: &mut i64, cx: &mut Cx<'_, '_, '_>) -> bool {
        block_memory(cx, MemoryKind::JobSite)
            .is_some_and(|site| closer_to_centre(site, cx.ai.position(), WORK_DISTANCE))
    }

    fn start(&self, _: &mut i64, cx: &mut Cx<'_, '_, '_>) {
        cx.memories
            .set(MemoryKind::LastWorkedAtPoi, Value::Long(cx.time));
        if let Some(site) = block_memory(cx, MemoryKind::JobSite) {
            cx.memories
                .set(MemoryKind::LookTarget, Value::Tracker(Tracker::Block(site)));
        }
        // `playWorkSound`: `makeSound` of the profession's, at the villager's voice.
        body::make_sound(cx.ai.tick, work_sound(profession(cx)), 1.0);
        if self.composter {
            use_composter(cx);
        }
        restock_if_due(cx);
    }
}

/// `WorkAtPoi` (work 5, every profession but the farmer's).
pub(crate) fn work_at_poi() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(WorkAtPoi { composter: false }))
}

/// `WorkAtComposter` (work 5, the farmer's).
pub(crate) fn work_at_composter() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(WorkAtPoi { composter: true }))
}

/// The most bread a farmer keeps before it bakes no more.
const BREAD_KEPT: i32 = 36;
/// What a farmer composts at most at once, and the seeds of each kind it keeps.
const COMPOST_AT_ONCE: i32 = 20;
const SEEDS_KEPT: i32 = 10;
/// `WorkAtComposter.COMPOSTABLE_ITEMS`.
const COMPOSTED: [i32; 2] = [items::WHEAT_SEEDS, items::BEETROOT_SEEDS];

/// How many of `item` the slots hold (`SimpleContainer.countItem`).
fn count_item(inventory: &[Option<ItemStack>; villager::SLOTS], item: i32) -> i32 {
    inventory
        .iter()
        .flatten()
        .filter(|stack| stack.item == item && stack.count > 0)
        .map(|stack| stack.count)
        .sum()
}

/// `SimpleContainer.removeItemType(item, count)`: from the last slot back, until `count`
/// are taken.
fn remove_item_type(inventory: &mut [Option<ItemStack>; villager::SLOTS], item: i32, count: i32) {
    let mut taken = 0;
    for slot in inventory.iter_mut().rev() {
        let Some(stack) = slot else {
            continue;
        };
        if stack.item != item {
            continue;
        }
        let split = (count - taken).min(stack.count).max(0);
        stack.count -= split;
        taken += split;
        if stack.count <= 0 {
            *slot = None;
        }
        if taken == count {
            break;
        }
    }
}

/// `WorkAtComposter.useWorkstation`: at a composter that is its job site, it bakes and
/// composts.
fn use_composter(cx: &mut Cx<'_, '_, '_>) {
    let Some(site) = block_memory(cx, MemoryKind::JobSite) else {
        return;
    };
    let state = cx.ai.tick.world.state(site);
    if !composting::is_composter(state) {
        return;
    }
    make_bread(cx);
    compost_items(cx, site, state);
}

/// `WorkAtComposter.makeBread`: with no more than 36 bread, the wheat by threes into up
/// to three bread; what the slots do not take is dropped at its feet half a block up.
fn make_bread(cx: &mut Cx<'_, '_, '_>) {
    let (_, own) = villager::state_of(&cx.ai.tick.mob);
    let mut inventory = own.inventory.clone();
    if count_item(&inventory, items::BREAD) > BREAD_KEPT {
        return;
    }
    let baked = (count_item(&inventory, items::WHEAT) / 3).min(3);
    if baked == 0 {
        return;
    }
    remove_item_type(&mut inventory, items::WHEAT, baked * 3);
    let bread = ItemStack::new(items::BREAD, baked);
    let kept = villager::add_item(&mut inventory, bread.clone());
    villager::update(&mut cx.ai.tick.mob, |own| own.inventory = inventory);
    if kept < baked {
        let [x, y, z] = cx.ai.position();
        cx.ai.tick.out.local.push(Local::Drop {
            stack: ItemStack {
                count: baked - kept,
                ..bread
            },
            at: [x, y + 0.5, z],
            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
        });
    }
}

/// `level.gameEvent(BLOCK_CHANGE, pos, Context.of(villager, state))`.
fn block_changed(cx: &mut Cx<'_, '_, '_>, pos: BlockPos, state: clustine_data::BlockState) {
    cx.ai.tick.out.local.push(Local::GameEventAt {
        name: "minecraft:block_change",
        at: centre(pos),
        affected: Some(state),
    });
}

/// The composter at `site` set to `state` with every update and its game event.
fn set_composter(cx: &mut Cx<'_, '_, '_>, site: BlockPos, state: clustine_data::BlockState) {
    cx.ai.tick.out.local.push(Local::SetBlock {
        pos: site,
        state,
        flags: composting::UPDATE_ALL,
    });
    block_changed(cx, site, state);
}

/// `WorkAtComposter.compostItems`: a ready composter emptied of its bone meal first;
/// then from the last slot back, the seeds of each kind beyond the first 10, 20 at most
/// in all, put in one by one (`ComposterBlock.insertItem`), stopping where it is full;
/// level event 1500 at the end, its data whether the level is other than it was after
/// the emptying.
fn compost_items(cx: &mut Cx<'_, '_, '_>, site: BlockPos, state: clustine_data::BlockState) {
    let mut before = state;
    if composting::level(state) == Some(composting::READY) {
        let (at, emptied) = composting::extract_produce(state, site, cx.ai.tick.draws);
        cx.ai.tick.out.local.push(Local::Drop {
            stack: ItemStack::new(items::BONE_MEAL, 1),
            at,
            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
        });
        set_composter(cx, site, emptied);
        cx.ai.tick.out.local.push(Local::Sound {
            sound: composting::EMPTY_SOUND,
            source: crate::api::SOUND_SOURCE_BLOCKS,
            at: centre(site),
            volume: 1.0,
            pitch: 1.0,
        });
        before = emptied;
    }
    let (_, own) = villager::state_of(&cx.ai.tick.mob);
    let mut inventory = own.inventory.clone();
    let mut left = COMPOST_AT_ONCE;
    let mut counts = [0; COMPOSTED.len()];
    let mut now = before;
    'slots: for slot in (0..villager::SLOTS).rev() {
        if left <= 0 {
            break;
        }
        let Some(stack) = inventory[slot].clone() else {
            continue;
        };
        let Some(index) = COMPOSTED.iter().position(|item| *item == stack.item) else {
            continue;
        };
        counts[index] += stack.count;
        let put = (counts[index] - SEEDS_KEPT).min(left).min(stack.count);
        if put <= 0 {
            continue;
        }
        left -= put;
        for _ in 0..put {
            let inserted = composting::insert_item(now, stack.item, cx.ai.tick.draws);
            if inserted.used
                && let Some(held) = inventory[slot].as_mut()
            {
                held.count -= 1;
                if held.count <= 0 {
                    inventory[slot] = None;
                }
            }
            if inserted.state != now {
                now = inserted.state;
                set_composter(cx, site, now);
            }
            if composting::level(now) == Some(composting::FULL) {
                break 'slots;
            }
        }
    }
    villager::update(&mut cx.ai.tick.mob, |own| own.inventory = inventory);
    cx.ai.tick.out.local.push(Local::LevelEvent {
        kind: composting::COMPOSTER_FILL,
        pos: site,
        data: i32::from(now != before),
    });
}

/// `HarvestFarmland` (Behavior 60/60; HarvestFarmland.txt 31–448), of a farmer while
/// `mob_griefing` is on (as everywhere): of the 27 blocks about it, ripe crops and air
/// over farmland, one at random; walked to; within 1 of it, a ripe crop is broken with
/// its drops, air over farmland gets the first of its seeds planted, and a crop that is
/// not ripe makes it pick another, 20 ticks on; 200 ticks of work at most.
struct HarvestFarmland;

/// What `HarvestFarmland` keeps: its place (`aboveFarmlandPos`), when it may start again
/// (`nextOkStartTime`), its ticks of work and the places it chose among.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Harvest {
    pub(crate) above: Option<BlockPos>,
    pub(crate) next_ok: i64,
    pub(crate) worked: i32,
    pub(crate) valid: Vec<BlockPos>,
}

/// The farmer's walking speed at work (`HarvestFarmland.SPEED_MODIFIER`).
const FARMING_SPEED: f32 = 0.5;

/// `CropBlock.isMaxAge` of the block at `pos`, none where it is no crop.
fn crop_grown_out_at(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> Option<bool> {
    composting::crop_grown_out(cx.ai.tick.world.state(pos))
}

/// `HarvestFarmland.validPos`: a ripe crop, or air over farmland.
fn harvestable(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    let world = cx.ai.tick.world;
    let state = world.state(pos);
    let below = world.state(BlockPos::new(pos.x, pos.y - 1, pos.z));
    composting::crop_grown_out(state) == Some(true)
        || (state.props().is_air()
            && clustine_blocks::data::is_class(below, BlockClass::FarmlandBlock))
}

/// `getValidFarmland`: one of the places at random (`nextInt(size)`), none of none.
fn pick_farmland(cx: &mut Cx<'_, '_, '_>, valid: &[BlockPos]) -> Option<BlockPos> {
    if valid.is_empty() {
        return None;
    }
    let at = cx.ai.tick.draws.next_int(valid.len() as u32) as usize;
    Some(valid[at])
}

/// Its look and walk at the place, close within 1.
fn farm_at(cx: &mut Cx<'_, '_, '_>, pos: BlockPos) {
    cx.memories
        .set(MemoryKind::LookTarget, Value::Tracker(Tracker::Block(pos)));
    cx.memories.set(
        MemoryKind::WalkTarget,
        walk(Tracker::Block(pos), FARMING_SPEED, 1),
    );
}

impl Timed for HarvestFarmland {
    type State = Harvest;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Absent),
            (MemoryKind::WalkTarget, Status::Absent),
            (MemoryKind::SecondaryJobSite, Status::Present),
        ]
    }

    fn extra_start(&self, state: &mut Harvest, cx: &mut Cx<'_, '_, '_>) -> bool {
        if profession(cx) != professions::FARMER {
            return false;
        }
        let [x, y, z] = cx.ai.position();
        state.valid.clear();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let pos = physics::containing([
                        x + f64::from(dx),
                        y + f64::from(dy),
                        z + f64::from(dz),
                    ]);
                    if harvestable(cx, pos) {
                        state.valid.push(pos);
                    }
                }
            }
        }
        let valid = state.valid.clone();
        state.above = pick_farmland(cx, &valid);
        state.above.is_some()
    }

    fn start(&self, state: &mut Harvest, cx: &mut Cx<'_, '_, '_>) {
        if cx.time > state.next_ok
            && let Some(pos) = state.above
        {
            farm_at(cx, pos);
        }
    }

    fn can_still_use(&self, state: &mut Harvest, _: &mut Cx<'_, '_, '_>) -> bool {
        state.worked < 200
    }

    fn tick(&self, state: &mut Harvest, cx: &mut Cx<'_, '_, '_>) {
        if let Some(pos) = state.above
            && !closer_to_centre(pos, cx.ai.position(), 1.0)
        {
            return;
        }
        if let Some(pos) = state.above
            && cx.time > state.next_ok
        {
            let world = cx.ai.tick.world;
            let block = world.state(pos);
            let below = world.state(BlockPos::new(pos.x, pos.y - 1, pos.z));
            let ripe = crop_grown_out_at(cx, pos);
            if ripe == Some(true) {
                // `level.destroyBlock(pos, true, villager)`.
                cx.ai.tick.out.local.push(Local::BreakBlock { pos });
            }
            if block.props().is_air()
                && clustine_blocks::data::is_class(below, BlockClass::FarmlandBlock)
            {
                plant(cx, pos);
            }
            if ripe == Some(false) {
                state.valid.retain(|valid| *valid != pos);
                let valid = state.valid.clone();
                state.above = pick_farmland(cx, &valid);
                if let Some(next) = state.above {
                    state.next_ok = cx.time + 20;
                    farm_at(cx, next);
                }
            }
        }
        state.worked += 1;
    }

    fn stop(&self, state: &mut Harvest, cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::LookTarget);
        cx.memories.erase(MemoryKind::WalkTarget);
        state.worked = 0;
        state.next_ok = cx.time + 40;
    }
}

/// `hasFarmSeeds()` and the planting of `HarvestFarmland.tick`: the first slot of
/// `#villager_plantable_seeds` that places a block plants it with every update, its
/// `block_place` game event and `crop_planted`'s sound at the block's corner, and is one
/// less.
fn plant(cx: &mut Cx<'_, '_, '_>, pos: BlockPos) {
    let (_, own) = villager::state_of(&cx.ai.tick.mob);
    let mut inventory = own.inventory.clone();
    let seeds = |stack: &ItemStack| {
        stack.count > 0 && item_in_tag(stack.item, "minecraft:villager_plantable_seeds")
    };
    if !inventory.iter().flatten().any(seeds) {
        return;
    }
    for slot in inventory.iter_mut() {
        let Some(stack) = slot.as_mut() else {
            continue;
        };
        if !seeds(stack) {
            continue;
        }
        let Some(block) = usize::try_from(stack.item)
            .ok()
            .and_then(|item| ITEMS.get(item))
            .and_then(|item| item.places)
        else {
            continue;
        };
        let planted = clustine_data::BLOCKS[usize::from(block)].default_state;
        cx.ai.tick.out.local.push(Local::SetBlock {
            pos,
            state: planted,
            flags: composting::UPDATE_ALL,
        });
        cx.ai.tick.out.local.push(Local::GameEventAt {
            name: "minecraft:block_place",
            at: centre(pos),
            affected: Some(planted),
        });
        cx.ai.tick.out.local.push(Local::Sound {
            sound: sounds::ITEM_CROP_PLANT,
            source: crate::api::SOUND_SOURCE_BLOCKS,
            at: [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)],
            volume: 1.0,
            pitch: 1.0,
        });
        stack.count -= 1;
        if stack.count <= 0 {
            *slot = None;
        }
        break;
    }
    villager::update(&mut cx.ai.tick.mob, |own| own.inventory = inventory);
}

/// `HarvestFarmland` (work 5).
pub(crate) fn harvest_farmland() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(HarvestFarmland))
}

/// `UseBonemeal` (Behavior 60/60; UseBonemeal.txt 27–287): every tenth tick of its life,
/// 160 after its last session, with bone meal: of the 27 blocks about it, a crop not
/// grown out chosen by reservoir (one `nextInt(count)` a crop); bone meal shown in hand;
/// within 1 of it, the first bone meal of its slots grows the crop (`growCrop`, its draws)
/// with the sparks of level event 1505, and the next crop is chosen, 40 ticks on; 80
/// ticks of work at most.
struct UseBonemeal;

/// What `UseBonemeal` keeps (`nextWorkCycleTime`, `lastBonemealingSession`,
/// `timeWorkedSoFar`, `cropPos`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Bonemealing {
    pub(crate) next_cycle: i64,
    pub(crate) last_session: i64,
    pub(crate) worked: i32,
    pub(crate) crop: Option<BlockPos>,
}

/// `UseBonemeal.validPos`: a crop not grown out.
fn bone_meal_target(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    crop_grown_out_at(cx, pos) == Some(false)
}

/// `pickNextTarget`: the 27 blocks about it (x, then y, then z), each crop kept with a
/// chance of one in how many were found so far.
fn pick_crop(cx: &mut Cx<'_, '_, '_>) -> Option<BlockPos> {
    let here = cx.ai.block_position();
    let mut chosen = None;
    let mut count = 0_u32;
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                let pos = here.offset(dx, dy, dz);
                if bone_meal_target(cx, pos) {
                    count += 1;
                    if cx.ai.tick.draws.next_int(count) == 0 {
                        chosen = Some(pos);
                    }
                }
            }
        }
    }
    chosen
}

/// `setCurrentCropAsTarget`.
fn crop_targeted(cx: &mut Cx<'_, '_, '_>, crop: Option<BlockPos>) {
    if let Some(pos) = crop {
        farm_at(cx, pos);
    }
}

/// The villager's main hand set (`setItemSlot(MAINHAND, stack)`).
fn hold(cx: &mut Cx<'_, '_, '_>, stack: Option<ItemStack>) {
    cx.ai.tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = stack;
}

impl Timed for UseBonemeal {
    type State = Bonemealing;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Absent),
            (MemoryKind::WalkTarget, Status::Absent),
        ]
    }

    fn extra_start(&self, state: &mut Bonemealing, cx: &mut Cx<'_, '_, '_>) -> bool {
        let lived = i64::from(cx.ai.tick.tick_count);
        if lived % 10 != 0 || (state.last_session != 0 && state.last_session + 160 > lived) {
            return false;
        }
        let (_, own) = villager::state_of(&cx.ai.tick.mob);
        if count_item(&own.inventory, items::BONE_MEAL) <= 0 {
            return false;
        }
        state.crop = pick_crop(cx);
        state.crop.is_some()
    }

    fn can_still_use(&self, state: &mut Bonemealing, _: &mut Cx<'_, '_, '_>) -> bool {
        state.worked < 80 && state.crop.is_some()
    }

    fn start(&self, state: &mut Bonemealing, cx: &mut Cx<'_, '_, '_>) {
        crop_targeted(cx, state.crop);
        hold(cx, Some(ItemStack::new(items::BONE_MEAL, 1)));
        state.next_cycle = cx.time;
        state.worked = 0;
    }

    fn stop(&self, state: &mut Bonemealing, cx: &mut Cx<'_, '_, '_>) {
        hold(cx, None);
        state.last_session = i64::from(cx.ai.tick.tick_count);
    }

    fn tick(&self, state: &mut Bonemealing, cx: &mut Cx<'_, '_, '_>) {
        let Some(pos) = state.crop else {
            return;
        };
        if cx.time < state.next_cycle || !closer_to_centre(pos, cx.ai.position(), 1.0) {
            return;
        }
        let (_, own) = villager::state_of(&cx.ai.tick.mob);
        let mut inventory = own.inventory.clone();
        let slot = inventory.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|stack| stack.item == items::BONE_MEAL && stack.count > 0)
        });
        if let Some(slot) = slot {
            // `BoneMealItem.growCrop`: a crop not grown out takes it, grows, and the bone
            // meal is used.
            let crop = cx.ai.tick.world.state(pos);
            if let Some(grown) = composting::bone_meal_crop(crop, cx.ai.tick.draws) {
                cx.ai.tick.out.local.push(Local::SetBlock {
                    pos,
                    state: grown,
                    flags: composting::UPDATE_CLIENTS,
                });
                if let Some(stack) = inventory[slot].as_mut() {
                    stack.count -= 1;
                    if stack.count <= 0 {
                        inventory[slot] = None;
                    }
                }
                villager::update(&mut cx.ai.tick.mob, |own| own.inventory = inventory);
                cx.ai.tick.out.local.push(Local::LevelEvent {
                    kind: composting::PLANT_GROWTH,
                    pos,
                    data: composting::PLANT_GROWTH_DATA,
                });
                state.crop = pick_crop(cx);
                crop_targeted(cx, state.crop);
                state.next_cycle = cx.time + 40;
            }
        }
        state.worked += 1;
    }
}

/// `UseBonemeal` (work 5).
pub(crate) fn use_bonemeal() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(UseBonemeal))
}

#[cfg(test)]
#[path = "work_n10_tests.rs"]
mod tests;
