//! The arrivals filed and not taken yet: entities sent across a line, kept for the region
//! that holds their chunk, not for the one that uses it (`docs/adr/0028-entities.md`,
//! section 5; `docs/groundwork/n3-plan.md`). They are filed as effects are, by number, by
//! chunk and by the segment their latest record is in, with an undo for a group that
//! cannot be made durable; unlike effects they are never dropped untaken.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeBounds;

use clustine_world::{Arrival, ChunkPos, Due, EffectId, Entity};

/// An arrival not taken yet, and the segment its latest record is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeptArrival {
    pub(crate) chunk: ChunkPos,
    pub(crate) entity: Entity,
    pub(crate) due: Due,
    pub(crate) segment: u64,
}

/// The arrivals filed and not taken yet.
#[derive(Default)]
pub(crate) struct FiledArrivals {
    arrivals: BTreeMap<EffectId, KeptArrival>,
    by_chunk: BTreeMap<ChunkPos, BTreeSet<EffectId>>,
    /// How many arrivals live in each segment.
    living: BTreeMap<u64, usize>,
}

impl FiledArrivals {
    /// Files an arrival, or moves it to the segment of a later record of it.
    pub(crate) fn file(&mut self, id: EffectId, kept: KeptArrival) {
        self.remove(id);
        self.by_chunk.entry(kept.chunk).or_default().insert(id);
        *self.living.entry(kept.segment).or_default() += 1;
        self.arrivals.insert(id, kept);
    }

    /// Takes the arrival out, if it is filed.
    pub(crate) fn remove(&mut self, id: EffectId) -> Option<KeptArrival> {
        let kept = self.arrivals.remove(&id)?;
        if let Some(ids) = self.by_chunk.get_mut(&kept.chunk) {
            ids.remove(&id);
            if ids.is_empty() {
                self.by_chunk.remove(&kept.chunk);
            }
        }
        if let Some(count) = self.living.get_mut(&kept.segment) {
            *count -= 1;
            if *count == 0 {
                self.living.remove(&kept.segment);
            }
        }
        Some(kept)
    }

    pub(crate) fn get(&self, id: EffectId) -> Option<&KeptArrival> {
        self.arrivals.get(&id)
    }

    /// Whether any arrival is filed for `chunk`.
    pub(crate) fn has(&self, chunk: ChunkPos) -> bool {
        self.by_chunk.contains_key(&chunk)
    }

    /// The numbers of the arrivals filed for `chunk`, ascending.
    pub(crate) fn ids_of(&self, chunk: ChunkPos) -> Vec<EffectId> {
        self.by_chunk
            .get(&chunk)
            .map_or_else(Vec::new, |ids| ids.iter().copied().collect())
    }

    /// The chunks that have arrivals filed, ascending.
    pub(crate) fn chunks(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.by_chunk.keys().copied()
    }

    /// The arrival as the stream carries it, if it is filed.
    pub(crate) fn filed(&self, id: EffectId) -> Option<Arrival> {
        let kept = self.arrivals.get(&id)?;
        Some(Arrival {
            id,
            chunk: kept.chunk,
            entity: kept.entity.clone(),
            due: kept.due,
        })
    }

    /// The arrivals filed for chunks `holds` says yes to, as the stream carries them,
    /// ascending by number.
    pub(crate) fn held(&self, mut holds: impl FnMut(ChunkPos) -> bool) -> Vec<Arrival> {
        let mut ids: Vec<EffectId> = self
            .by_chunk
            .iter()
            .filter(|(chunk, _)| holds(**chunk))
            .flat_map(|(_, ids)| ids.iter().copied())
            .collect();
        ids.sort_unstable();
        ids.into_iter().filter_map(|id| self.filed(id)).collect()
    }

    /// An arrival filed for one of `chunks` is due at once from now on: its chunk's holder
    /// changed, whose ticks are not those its `Due` was made in (the plan, section 6).
    pub(crate) fn count_from_taking(&mut self, chunks: &[ChunkPos]) {
        for chunk in chunks {
            for id in self.by_chunk.get(chunk).into_iter().flatten() {
                if let Some(kept) = self.arrivals.get_mut(id) {
                    kept.due = Due::FromTaking;
                }
            }
        }
    }

    /// Every arrival filed below `sequence` is due at once from now on: for a store that
    /// reads its log at a start, from a record of a merge or a split in that place.
    pub(crate) fn count_from_taking_below(&mut self, sequence: u64) {
        for (id, kept) in &mut self.arrivals {
            if id.sequence < sequence {
                kept.due = Due::FromTaking;
            }
        }
    }

    /// Whether an arrival lives in `segment`.
    pub(crate) fn lives_in(&self, segment: u64) -> bool {
        self.living.contains_key(&segment)
    }

    /// The arrivals that live in the segments of `segments`, ascending by number.
    pub(crate) fn living_in(&self, segments: impl RangeBounds<u64>) -> Vec<Arrival> {
        let segments: BTreeSet<u64> = self.living.range(segments).map(|(s, _)| *s).collect();
        if segments.is_empty() {
            return Vec::new();
        }
        self.arrivals
            .iter()
            .filter(|(_, kept)| segments.contains(&kept.segment))
            .filter_map(|(id, _)| self.filed(*id))
            .collect()
    }

    /// Every arrival filed, ascending by number.
    pub(crate) fn all(&self) -> Vec<Arrival> {
        self.arrivals
            .keys()
            .filter_map(|id| self.filed(*id))
            .collect()
    }
}

/// Something a group did to the arrivals filed, taken back, last first, if the group
/// cannot be made durable.
#[expect(
    clippy::large_enum_variant,
    reason = "an entity is held whole while the store reads, files or replays it; boxing it would allocate once more for each"
)]
pub(crate) enum ArrivalUndo {
    Filed(EffectId),
    Removed(EffectId, KeptArrival),
}

impl FiledArrivals {
    pub(crate) fn undo(&mut self, undo: Vec<ArrivalUndo>) {
        for step in undo.into_iter().rev() {
            match step {
                ArrivalUndo::Filed(id) => {
                    self.remove(id);
                }
                ArrivalUndo::Removed(id, kept) => self.file(id, kept),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_world::{EntityData, EntityId, ItemStack};

    use super::*;

    fn item(key: u64, x: f64) -> Entity {
        Entity {
            key: EntityId(key),
            kind: 71,
            uuid: uuid::Uuid::from_u128(u128::from(key)),
            position: [x, -60.0, 0.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            hops: 0,
            age: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::Item {
                stack: ItemStack { item: 1, count: 1 },
                pick_up_delay: 0,
                thrower: None,
                health: 5,
            },
        }
    }

    fn kept(key: u64, x: f64, segment: u64) -> KeptArrival {
        let entity = item(key, x);
        KeptArrival {
            chunk: entity.chunk(),
            entity,
            due: Due::At { tick: 9 },
            segment,
        }
    }

    fn id(sequence: u64, index: u32) -> EffectId {
        EffectId { sequence, index }
    }

    #[test]
    fn arrivals_are_found_by_chunk_and_segment_and_go_when_taken() {
        let mut filed = FiledArrivals::default();
        filed.file(id(3, 0), kept(1, 16.5, 4));
        filed.file(id(3, 1), kept(2, 0.5, 4));
        filed.file(id(5, 0), kept(3, 17.5, 6));
        let east = ChunkPos::new(1, 0);
        assert_eq!(filed.ids_of(east), [id(3, 0), id(5, 0)]);
        assert!(filed.has(ChunkPos::new(0, 0)));
        let held: Vec<EffectId> = filed
            .held(|chunk| chunk == east)
            .iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(held, [id(3, 0), id(5, 0)]);
        assert!(filed.lives_in(4) && filed.lives_in(6));
        // Written forward: the same number in a later segment.
        filed.file(id(3, 0), kept(1, 16.5, 7));
        assert_eq!(filed.living_in(..5).len(), 1);
        assert_eq!(
            filed.remove(id(3, 1)).map(|kept| kept.entity.key),
            Some(EntityId(2))
        );
        assert!(!filed.lives_in(4));
        assert!(!filed.has(ChunkPos::new(0, 0)));
        assert_eq!(filed.all().len(), 2);
    }

    #[test]
    fn a_change_of_holder_makes_arrivals_due_at_once_and_an_undo_takes_a_group_back() {
        let mut filed = FiledArrivals::default();
        filed.file(id(3, 0), kept(1, 16.5, 4));
        filed.file(id(8, 0), kept(2, 0.5, 4));
        filed.count_from_taking(&[ChunkPos::new(1, 0)]);
        assert_eq!(
            filed.get(id(3, 0)).map(|kept| kept.due),
            Some(Due::FromTaking)
        );
        assert_eq!(
            filed.get(id(8, 0)).map(|kept| kept.due),
            Some(Due::At { tick: 9 })
        );
        filed.count_from_taking_below(9);
        assert_eq!(
            filed.get(id(8, 0)).map(|kept| kept.due),
            Some(Due::FromTaking)
        );

        let removed = filed.remove(id(3, 0)).expect("filed");
        filed.file(id(9, 0), kept(3, 33.5, 5));
        filed.undo(vec![
            ArrivalUndo::Removed(id(3, 0), removed),
            ArrivalUndo::Filed(id(9, 0)),
        ]);
        let ids: Vec<EffectId> = filed.all().iter().map(|arrival| arrival.id).collect();
        assert_eq!(ids, [id(3, 0), id(8, 0)]);
    }
}
