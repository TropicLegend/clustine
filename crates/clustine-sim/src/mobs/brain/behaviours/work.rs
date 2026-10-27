//! The villager's points of interest and work (`docs/groundwork/n10-plan.md`, section 6):
//! claiming job sites, beds and the bell, keeping and losing them, professions, walking to
//! points and working there. Part V2 fills it. The contract step N10c declared each
//! behaviour's constructor with the game's arguments, inert, so that part V1 can name them
//! in the villager's packages while V2 builds them.

#![allow(dead_code, reason = "part V1 names these in the villager's packages")]

use crate::mobs::brain::{Behaviour, MemoryKind, inert};

/// `AcquirePoi.create(acquirableJobSite, job_site, potential_job_site, true, empty)`
/// (core 6): an adult without a job site reserves one.
pub(crate) fn acquire_job_site() -> Box<dyn Behaviour> {
    inert()
}

/// `AcquirePoi.create(HOME, home, false, Optional.of(14), validateBedPoi)` (core 10).
pub(crate) fn acquire_home() -> Box<dyn Behaviour> {
    inert()
}

/// `AcquirePoi.create(MEETING, meeting_point, true, Optional.of(14))` (core 10).
pub(crate) fn acquire_meeting_point() -> Box<dyn Behaviour> {
    inert()
}

/// `ValidateNearbyPoi.create(profession.heldJobSite, job_site)` (core 0), of the villager's
/// profession by its registry id, as the brain was made.
pub(crate) fn validate_job_site(profession: u8) -> Box<dyn Behaviour> {
    let _ = profession;
    inert()
}

/// `ValidateNearbyPoi.create(acquirableJobSite, potential_job_site)` (core 0).
pub(crate) fn validate_potential_job_site() -> Box<dyn Behaviour> {
    inert()
}

/// `ValidateNearbyPoi.create(HOME, home)` (rest 3).
pub(crate) fn validate_home() -> Box<dyn Behaviour> {
    inert()
}

/// `ValidateNearbyPoi.create(MEETING, meeting_point)` (meet 3).
pub(crate) fn validate_meeting_point() -> Box<dyn Behaviour> {
    inert()
}

/// `PoiCompetitorScan.create()` (core 2).
pub(crate) fn poi_competitor_scan() -> Box<dyn Behaviour> {
    inert()
}

/// `GoToPotentialJobSite(speed)` (core 7).
pub(crate) fn go_to_potential_job_site(speed: f32) -> Box<dyn Behaviour> {
    let _ = speed;
    inert()
}

/// `YieldJobSite.create(speed)` (core 8).
pub(crate) fn yield_job_site(speed: f32) -> Box<dyn Behaviour> {
    let _ = speed;
    inert()
}

/// `AssignProfessionFromJobSite.create()` (core 10).
pub(crate) fn assign_profession_from_job_site() -> Box<dyn Behaviour> {
    inert()
}

/// `ResetProfession.create()` (core 10).
pub(crate) fn reset_profession() -> Box<dyn Behaviour> {
    inert()
}

/// `SetWalkTargetFromBlockMemory.create(memory, speed, close, tooFar, tooLong)`.
pub(crate) fn set_walk_target_from_block_memory(
    memory: MemoryKind,
    speed: f32,
    close: i32,
    too_far: i32,
    too_long: i32,
) -> Box<dyn Behaviour> {
    let _ = (memory, speed, close, too_far, too_long);
    inert()
}

/// `StrollAroundPoi.create(memory, speed, maxDistance)`.
pub(crate) fn stroll_around_poi(
    memory: MemoryKind,
    speed: f32,
    max_distance: i32,
) -> Box<dyn Behaviour> {
    let _ = (memory, speed, max_distance);
    inert()
}

/// `StrollToPoi.create(memory, speed, close, maxDistance)`.
pub(crate) fn stroll_to_poi(
    memory: MemoryKind,
    speed: f32,
    close: i32,
    max_distance: i32,
) -> Box<dyn Behaviour> {
    let _ = (memory, speed, close, max_distance);
    inert()
}

/// `StrollToPoiList.create(list, speed, close, maxDistance, memory)`.
pub(crate) fn stroll_to_poi_list(
    list: MemoryKind,
    speed: f32,
    close: i32,
    max_distance: i32,
    memory: MemoryKind,
) -> Box<dyn Behaviour> {
    let _ = (list, speed, close, max_distance, memory);
    inert()
}

/// `WorkAtPoi` (work 5, every profession but the farmer's).
pub(crate) fn work_at_poi() -> Box<dyn Behaviour> {
    inert()
}

/// `WorkAtComposter` (work 5, the farmer's).
pub(crate) fn work_at_composter() -> Box<dyn Behaviour> {
    inert()
}

/// `HarvestFarmland` (work 5).
pub(crate) fn harvest_farmland() -> Box<dyn Behaviour> {
    inert()
}

/// `UseBonemeal` (work 5).
pub(crate) fn use_bonemeal() -> Box<dyn Behaviour> {
    inert()
}
