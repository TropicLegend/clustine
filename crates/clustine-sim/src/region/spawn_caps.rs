//! Natural spawning's frame (`docs/groundwork/n9-plan.md`, sections 9.1, 9.2 and 9.5):
//! the region's counts of its mobs by spawning category, reported to the store with its
//! commits; the caps, from the region's own counts now and what the store last published
//! of the other regions'; and the nearest player for despawning and spawning, from the
//! region's players, the faces it keeps and the world's players' table.
//!
//! **The counts.** For each chunk the region has loaded (not a copy), the mobs of each of
//! the seven categories (`MobCategory` but `MISC`), counting what the game's
//! `NaturalSpawner.createState` counts: not a mob that is persistent (`isPersistenceRequired`
//! or `requiresCustomPersistence`: one that rides, is leashed, or whose type keeps it).
//! A tick reports each chunk whose counts changed, whole, and a chunk let go as nothing;
//! the region's first tick after it is made or restored (a merge, a split, a take-over)
//! reports every loaded chunk's.

use std::collections::BTreeMap;

use clustine_data::{EntityTypeInfo, MobCategory};
use clustine_world::{ChunkPos, Entity, EntityData};

use super::Region;
use crate::api::{CountsReport, SPAWN_CATEGORIES, SpawnCounts, TickOutput};
use crate::mobs::Mob;

/// The index of `category` among the counted ones, in the game's order; none for
/// `MISC`.
pub(crate) fn category_index(category: MobCategory) -> Option<usize> {
    let index = match category {
        MobCategory::Monster => 0,
        MobCategory::Creature => 1,
        MobCategory::Ambient => 2,
        MobCategory::Axolotls => 3,
        MobCategory::UndergroundWaterCreature => 4,
        MobCategory::WaterCreature => 5,
        MobCategory::WaterAmbient => 6,
        MobCategory::Misc => return None,
    };
    debug_assert!(index < SPAWN_CATEGORIES);
    Some(index)
}

/// The category `entity` counts in, if it counts: a mob not kept by anything.
fn counted_in(entity: &Entity) -> Option<usize> {
    let EntityData::Mob { .. } = entity.data else {
        return None;
    };
    let index = category_index(EntityTypeInfo::of(i32::from(entity.kind)).category)?;
    let mob = Mob::decode(&entity.data).ok()?;
    let kept = mob.body.flags.persistent
        || entity.riding.vehicle.is_some()
        || entity.riding.leash.is_some()
        || crate::mobs::types::of(entity.kind).requires_custom_persistence(&mob);
    (!kept).then_some(index)
}

/// The counts of `entities`, a chunk's.
fn counts_of(entities: &[Entity]) -> SpawnCounts {
    let mut counts = [0; SPAWN_CATEGORIES];
    for index in entities.iter().filter_map(counted_in) {
        counts[index] += 1;
    }
    counts
}

/// What the region keeps of its counts between ticks: each loaded chunk's as last
/// reported. In memory only: a restored region reports every chunk again.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Counts {
    pub(crate) by_chunk: BTreeMap<ChunkPos, SpawnCounts>,
}

impl Region {
    /// The tick's report of its counts (section 9.2), at the end of the tick: every
    /// loaded chunk's where `full`, else each chunk whose counts changed, and nothing for
    /// a chunk let go.
    pub(super) fn report_counts(&mut self, full: bool, output: &mut TickOutput) {
        let mut chunks: Vec<(ChunkPos, SpawnCounts)> = Vec::new();
        let gone: Vec<ChunkPos> = self
            .counts
            .by_chunk
            .keys()
            .copied()
            .filter(|position| !self.chunks.contains_key(position))
            .collect();
        for position in gone {
            self.counts.by_chunk.remove(&position);
            chunks.push((position, [0; SPAWN_CATEGORIES]));
        }
        // Counted again only where the tick changed entities, or the chunk is newly
        // loaded: a mob's record is decoded to tell whether it is kept.
        let touched: std::collections::BTreeSet<ChunkPos> = output
            .entity_changes
            .iter()
            .map(|(position, _)| *position)
            .collect();
        for (position, chunk) in &self.chunks {
            let known = self.counts.by_chunk.get(position).copied();
            let now = match known {
                Some(counts) if !touched.contains(position) => counts,
                _ => counts_of(chunk.entities()),
            };
            self.counts.by_chunk.insert(*position, now);
            if full || known != Some(now) {
                chunks.push((*position, now));
            }
        }
        chunks.sort_by_key(|(position, _)| *position);
        if full || !chunks.is_empty() {
            output.counts = Some(CountsReport { full, chunks });
        }
    }
}
