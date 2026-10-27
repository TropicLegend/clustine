//! The villager's own sensors (`docs/groundwork/n10-brief.md`, section 1.1, cited as V;
//! `n10-plan.md`, section 5): the baby's nearest bed (`NearestBedSensor`), the babies it
//! sees (`VillagerBabiesSensor`), the blocks about it its profession works at besides its
//! site (`SecondaryPoiSensor`) and an iron golem seen lately (`GolemSensor`).
//!
//! Every scan but the bed's draws nothing; the bed's draws the level's `nextInt(20)` for
//! a baby at each scan, on the villager's stream.
//!
//! **Named**: the bed sensor's cache of the beds it tried is the game's sensor's own,
//! made afresh with each brain; here it is kept in the villager's state and made afresh
//! as the villager grows up, but not at a change of profession (`work.rs`'s), where it
//! may hold a bed for at most 40 more ticks.

use clustine_data::entity_types;
use clustine_world::BlockPos;

use super::sensors::{self, visible_mobs};
use super::{Cx, MemoryKind, Sensor, Value};
use crate::mobs::brain::behaviours::rest::search_beds;
use crate::mobs::types::villager;

/// `NearestBedSensor` (`NEAREST_BED`, a scan every 20 ticks; V 1.5): a baby's scan draws
/// its search's time (the level's `nextInt(20)`), tries up to four beds within 48 it has
/// not tried in the last 40 ticks by one path search, and remembers the bed it reaches
/// (none is forgotten where it reaches none); an adult's does nothing. Its sensing again
/// after a take-in draws nothing.
pub(crate) struct NearestBedSensor;

pub(crate) static NEAREST_BED: NearestBedSensor = NearestBedSensor;

impl Sensor for NearestBedSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::NearestBed]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let tick = &*cx.ai.tick;
        if !tick.mob_type.is_baby(&tick.mob) {
            return;
        }
        let mut search = villager::state_of(&cx.ai.tick.mob).1.bed_search;
        let found = search_beds(cx, &mut search.cache, &mut search.last_update);
        villager::update(&mut cx.ai.tick.mob, |own| own.bed_search = search);
        if let Some(bed) = found {
            cx.memories.set(MemoryKind::NearestBed, Value::Block(bed));
        }
    }

    fn resense(&self, _: &mut Cx<'_, '_, '_>) {}
}

/// `VillagerBabiesSensor` (`VILLAGER_BABIES`, every 20 ticks): of `visible_mobs`, every
/// villager baby it sees, nearest first, as `visible_villager_babies` (none forgotten as
/// an empty list).
pub(crate) struct VillagerBabiesSensor;

pub(crate) static VILLAGER_BABIES: VillagerBabiesSensor = VillagerBabiesSensor;

impl Sensor for VillagerBabiesSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::VisibleVillagerBabies]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        // `NearestVisibleLivingEntities.findAll(predicate)`: each of the list that passes
        // and is seen, in its order.
        let list = visible_mobs(cx.memories).unwrap_or_default();
        let mut babies = Vec::new();
        for key in list {
            let Some(looked) = sensors::find(cx.ai, key) else {
                continue;
            };
            let villager_baby = looked.kind == Some(entity_types::VILLAGER as u16)
                && sensors::is_baby(cx.ai, &looked);
            if villager_baby && sensors::targetable(cx, &looked) {
                babies.push(key);
            }
        }
        cx.memories
            .set(MemoryKind::VisibleVillagerBabies, Value::Entities(babies));
    }
}

/// `SecondaryPoiSensor` (`SECONDARY_POIS`, every 40 ticks): the blocks within 4 across
/// and 2 up and down of its feet (x outermost, then y, then z) of a block its profession
/// works at besides its site (`VillagerProfession.secondaryPoi`: farmland for a farmer,
/// none for any other), as `secondary_job_site`, or forgotten where there is none.
pub(crate) struct SecondaryPoiSensor;

pub(crate) static SECONDARY_POIS: SecondaryPoiSensor = SecondaryPoiSensor;

/// `VillagerProfession.secondaryPoi()` of a profession by its registry id: the farmer's
/// farmland (VillagerProfession.txt 331–377), nothing for every other.
fn secondary_poi(profession: u8, state: clustine_data::BlockState) -> bool {
    i32::from(profession) == clustine_data::villagers::professions::FARMER
        && clustine_blocks::data::is_block(state, clustine_data::blocks::FARMLAND)
}

impl Sensor for SecondaryPoiSensor {
    fn rate(&self) -> u32 {
        40
    }

    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::SecondaryJobSite]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let profession = villager::state_of(&cx.ai.tick.mob).1.data.profession;
        let at = cx.ai.block_position();
        let world = cx.ai.tick.world;
        let mut found = Vec::new();
        for dx in -4..=4 {
            for dy in -2..=2 {
                for dz in -4..=4 {
                    let pos = BlockPos::new(at.x + dx, at.y + dy, at.z + dz);
                    if secondary_poi(profession, world.state(pos)) {
                        found.push(pos);
                    }
                }
            }
        }
        if found.is_empty() {
            cx.memories.erase(MemoryKind::SecondaryJobSite);
        } else {
            cx.memories
                .set(MemoryKind::SecondaryJobSite, Value::Blocks(found));
        }
    }

    fn resense(&self, _: &mut Cx<'_, '_, '_>) {}
}

/// `GolemSensor` (`GOLEM_DETECTED`, every 200 ticks): an iron golem among the living near
/// it (`mobs`, seen or not) keeps `golem_detected_recently` for 600 ticks
/// (`setMemoryWithExpiry(…, 599)`); nothing where the list is not there.
pub(crate) struct GolemSensor;

pub(crate) static GOLEM_DETECTED: GolemSensor = GolemSensor;

/// `GolemSensor.golemDetected(villager)`: `golem_detected_recently` for 600 ticks (a
/// golem summoned near it, part V3, sets it too).
pub(crate) fn golem_detected(memories: &mut super::Memories) {
    memories.set_with_expiry(MemoryKind::GolemDetectedRecently, Value::Bool(true), 599);
}

impl Sensor for GolemSensor {
    fn rate(&self) -> u32 {
        200
    }

    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::Mobs, MemoryKind::GolemDetectedRecently]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let Some(Value::Entities(list)) = cx.memories.get(MemoryKind::Mobs) else {
            return;
        };
        let golem = list.into_iter().any(|key| {
            sensors::find(cx.ai, key)
                .is_some_and(|looked| looked.kind == Some(entity_types::IRON_GOLEM as u16))
        });
        if golem {
            golem_detected(cx.memories);
        }
    }

    fn resense(&self, _: &mut Cx<'_, '_, '_>) {}
}
