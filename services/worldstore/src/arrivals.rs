//! The arrivals filed and not taken yet: entities sent across a line, kept for the region
//! that holds their chunk, not for the one that uses it (`docs/adr/0028-entities.md`,
//! section 5; `docs/groundwork/n3-plan.md`). They are filed as effects are, by number, by
//! chunk and by the segment their latest record is in, with an undo for a group that
//! cannot be made durable; unlike effects they are never dropped untaken.
//!
//! Travellers are filed here too: entities other than players that a portal sent on,
//! each for the chunk it is aimed at, in whatever dimension that is
//! (`docs/groundwork/p-plan.md`, sections 5 and 15.4). They share the arrivals' numbers,
//! are always due at once, and are taken as arrivals are. Unlike an arrival, a
//! traveller is never folded into a chunk: it is written forward instead.
//!
//! A chunk is named by its dimension and its position. An arrival is of the dimension
//! of the region that sent it, whose commit has it; the record that writes arrivals
//! forward names their dimension (`LogRecord::ArrivalsForward`), one record for the
//! arrivals of each. A traveller names its own.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeBounds;

use clustine_world::{Arrival, Dimension, Due, EffectId, Entity, LevelChunk, Traveller};

/// What is filed: an entity sent across a line, or a traveller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Carried {
    Entity(Entity),
    Traveller(Traveller),
}

/// An arrival not taken yet, and the segment its latest record is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeptArrival {
    pub(crate) chunk: LevelChunk,
    pub(crate) carried: Carried,
    pub(crate) due: Due,
    pub(crate) segment: u64,
}

impl KeptArrival {
    /// An entity sent across a line into `chunk`.
    pub(crate) fn entity(chunk: LevelChunk, entity: Entity, due: Due, segment: u64) -> Self {
        Self {
            chunk,
            carried: Carried::Entity(entity),
            due,
            segment,
        }
    }

    /// A traveller, filed for the chunk it names and due at once: the region that
    /// takes it places it, which no tick of the sender's waits for.
    pub(crate) fn traveller(traveller: Traveller, segment: u64) -> Self {
        Self {
            chunk: traveller.chunk,
            carried: Carried::Traveller(traveller),
            due: Due::FromTaking,
            segment,
        }
    }
}

/// What a region's stream carries of what is filed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "an entity is held whole while the store reads, files or replays it; boxing it would allocate once more for each"
)]
pub(crate) enum Handed {
    Arrival(Arrival),
    Traveller(EffectId, Traveller),
}

/// What is filed for a set of chunks or segments, as streams and records carry it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Gathered {
    pub(crate) arrivals: Vec<Arrival>,
    pub(crate) travellers: Vec<(EffectId, Traveller)>,
}

impl Gathered {
    pub(crate) fn is_empty(&self) -> bool {
        self.arrivals.is_empty() && self.travellers.is_empty()
    }

    fn add(&mut self, handed: Handed) {
        match handed {
            Handed::Arrival(arrival) => self.arrivals.push(arrival),
            Handed::Traveller(id, traveller) => self.travellers.push((id, traveller)),
        }
    }
}

/// What a segment's arrivals and travellers are written forward as.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Forward {
    pub(crate) arrivals: Vec<(Dimension, Arrival)>,
    pub(crate) travellers: Vec<(EffectId, Traveller)>,
}

impl Forward {
    pub(crate) fn is_empty(&self) -> bool {
        self.arrivals.is_empty() && self.travellers.is_empty()
    }
}

/// The arrivals filed and not taken yet.
#[derive(Default)]
pub(crate) struct FiledArrivals {
    arrivals: BTreeMap<EffectId, KeptArrival>,
    by_chunk: BTreeMap<LevelChunk, BTreeSet<EffectId>>,
    /// How many arrivals live in each segment.
    living: BTreeMap<u64, usize>,
}

/// Counts one more in `segment`.
fn count_in(counts: &mut BTreeMap<u64, usize>, segment: u64) {
    *counts.entry(segment).or_default() += 1;
}

/// Counts one fewer in `segment`.
fn count_out(counts: &mut BTreeMap<u64, usize>, segment: u64) {
    if let Some(count) = counts.get_mut(&segment) {
        *count -= 1;
        if *count == 0 {
            counts.remove(&segment);
        }
    }
}

impl FiledArrivals {
    /// Files an arrival, or moves it to the segment of a later record of it.
    pub(crate) fn file(&mut self, id: EffectId, kept: KeptArrival) {
        self.remove(id);
        self.by_chunk.entry(kept.chunk).or_default().insert(id);
        count_in(&mut self.living, kept.segment);
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
        count_out(&mut self.living, kept.segment);
        Some(kept)
    }

    pub(crate) fn get(&self, id: EffectId) -> Option<&KeptArrival> {
        self.arrivals.get(&id)
    }

    /// Whether any arrival is filed for `chunk`.
    pub(crate) fn has(&self, chunk: LevelChunk) -> bool {
        self.by_chunk.contains_key(&chunk)
    }

    /// The numbers of the arrivals filed for `chunk`, ascending.
    pub(crate) fn ids_of(&self, chunk: LevelChunk) -> Vec<EffectId> {
        self.by_chunk
            .get(&chunk)
            .map_or_else(Vec::new, |ids| ids.iter().copied().collect())
    }

    /// The chunks that have arrivals filed, ascending.
    pub(crate) fn chunks(&self) -> impl Iterator<Item = LevelChunk> + '_ {
        self.by_chunk.keys().copied()
    }

    /// The arrival or the traveller as the stream carries it, if it is filed.
    pub(crate) fn filed(&self, id: EffectId) -> Option<Handed> {
        let kept = self.arrivals.get(&id)?;
        Some(match &kept.carried {
            Carried::Entity(entity) => Handed::Arrival(Arrival {
                id,
                chunk: kept.chunk.position,
                entity: entity.clone(),
                due: kept.due,
            }),
            Carried::Traveller(traveller) => Handed::Traveller(id, traveller.clone()),
        })
    }

    /// What is filed for chunks `holds` says yes to, as the stream carries it,
    /// ascending by number.
    pub(crate) fn held(&self, mut holds: impl FnMut(LevelChunk) -> bool) -> Gathered {
        let mut ids: Vec<EffectId> = self
            .by_chunk
            .iter()
            .filter(|(chunk, _)| holds(**chunk))
            .flat_map(|(_, ids)| ids.iter().copied())
            .collect();
        ids.sort_unstable();
        let mut gathered = Gathered::default();
        for handed in ids.into_iter().filter_map(|id| self.filed(id)) {
            gathered.add(handed);
        }
        gathered
    }

    /// An arrival filed for one of `chunks` is due at once from now on: its chunk's holder
    /// changed, whose ticks are not those its `Due` was made in (the plan, section 6).
    pub(crate) fn count_from_taking(&mut self, chunks: &[LevelChunk]) {
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

    /// What lives in the segments of `segments`, ascending by number: what is written
    /// forward, each arrival with its dimension.
    pub(crate) fn living_in(&self, segments: impl RangeBounds<u64>) -> Forward {
        let segments: BTreeSet<u64> = self.living.range(segments).map(|(s, _)| *s).collect();
        let mut forward = Forward::default();
        if segments.is_empty() {
            return forward;
        }
        let kept = self
            .arrivals
            .iter()
            .filter(|(_, kept)| segments.contains(&kept.segment));
        for (id, kept) in kept {
            match self.filed(*id) {
                Some(Handed::Arrival(arrival)) => {
                    forward.arrivals.push((kept.chunk.dimension, arrival));
                }
                Some(Handed::Traveller(id, traveller)) => forward.travellers.push((id, traveller)),
                None => {}
            }
        }
        forward
    }

    /// Every arrival of an entity filed, ascending by number, with its dimension:
    /// travellers are not among them.
    pub(crate) fn all(&self) -> Vec<(Arrival, Dimension)> {
        self.arrivals
            .iter()
            .filter_map(|(id, kept)| match self.filed(*id)? {
                Handed::Arrival(arrival) => Some((arrival, kept.chunk.dimension)),
                Handed::Traveller(..) => None,
            })
            .collect()
    }

    /// Every traveller filed, ascending by number.
    pub(crate) fn travellers(&self) -> Vec<(EffectId, Traveller)> {
        self.arrivals
            .iter()
            .filter_map(|(id, kept)| match &kept.carried {
                Carried::Traveller(traveller) => Some((*id, traveller.clone())),
                Carried::Entity(_) => None,
            })
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
    use clustine_world::{
        BlockPos, ChunkPos, Double, EntityData, EntityId, ItemStack, Travel, Way,
    };

    use super::*;

    fn item(key: u64, x: f64) -> Entity {
        Entity {
            portal: Default::default(),
            key: EntityId(key),
            kind: 71,
            uuid: uuid::Uuid::from_u128(u128::from(key)),
            position: [x, -60.0, 0.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::Item {
                stack: ItemStack::new(1, 1),
                pick_up_delay: 0,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
        }
    }

    fn overworld(position: ChunkPos) -> LevelChunk {
        LevelChunk::new(Dimension::Overworld, position)
    }

    fn kept_in(dimension: Dimension, key: u64, x: f64, segment: u64) -> KeptArrival {
        let entity = item(key, x);
        let chunk = LevelChunk::new(dimension, entity.chunk());
        KeptArrival::entity(chunk, entity, Due::At { tick: 9 }, segment)
    }

    fn kept(key: u64, x: f64, segment: u64) -> KeptArrival {
        kept_in(Dimension::Overworld, key, x, segment)
    }

    /// A traveller for the Nether's chunk at the block `x`.
    fn traveller(key: u64, x: i32) -> Traveller {
        let target = BlockPos::new(x, 70, 0);
        Traveller {
            travel: Travel {
                to: Dimension::Nether,
                target,
                way: Way::NetherPortal {
                    axis: None,
                    relative: [Double(0.5), Double(0.0), Double(0.5)],
                },
            },
            chunk: LevelChunk::new(Dimension::Nether, target.chunk()),
            entities: vec![item(key, f64::from(x))],
        }
    }

    fn id(sequence: u64, index: u32) -> EffectId {
        EffectId { sequence, index }
    }

    fn ids(arrivals: &[Arrival]) -> Vec<EffectId> {
        arrivals.iter().map(|arrival| arrival.id).collect()
    }

    #[test]
    fn arrivals_are_found_by_chunk_and_segment_and_go_when_taken() {
        let mut filed = FiledArrivals::default();
        filed.file(id(3, 0), kept(1, 16.5, 4));
        filed.file(id(3, 1), kept(2, 0.5, 4));
        filed.file(id(5, 0), kept(3, 17.5, 6));
        let east = overworld(ChunkPos::new(1, 0));
        assert_eq!(filed.ids_of(east), [id(3, 0), id(5, 0)]);
        assert!(filed.has(overworld(ChunkPos::new(0, 0))));
        let held = filed.held(|chunk| chunk == east);
        assert_eq!(ids(&held.arrivals), [id(3, 0), id(5, 0)]);
        assert!(filed.lives_in(4) && filed.lives_in(6));
        // Written forward: the same number in a later segment.
        filed.file(id(3, 0), kept(1, 16.5, 7));
        assert_eq!(filed.living_in(..5).arrivals.len(), 1);
        assert_eq!(
            filed.remove(id(3, 1)).map(|kept| kept.carried),
            Some(Carried::Entity(item(2, 0.5)))
        );
        assert!(!filed.lives_in(4));
        assert!(!filed.has(overworld(ChunkPos::new(0, 0))));
        assert_eq!(filed.all().len(), 2);
    }

    #[test]
    fn a_change_of_holder_makes_arrivals_due_at_once_and_an_undo_takes_a_group_back() {
        let mut filed = FiledArrivals::default();
        filed.file(id(3, 0), kept(1, 16.5, 4));
        filed.file(id(8, 0), kept(2, 0.5, 4));
        filed.count_from_taking(&[overworld(ChunkPos::new(1, 0))]);
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
        let all: Vec<EffectId> = filed.all().iter().map(|(arrival, _)| arrival.id).collect();
        assert_eq!(all, [id(3, 0), id(8, 0)]);
    }

    #[test]
    fn the_same_position_in_two_dimensions_is_two_chunks() {
        let mut filed = FiledArrivals::default();
        filed.file(id(3, 0), kept(1, 16.5, 4));
        filed.file(id(3, 1), kept_in(Dimension::Nether, 2, 16.5, 4));
        let position = ChunkPos::new(1, 0);
        assert_eq!(filed.ids_of(overworld(position)), [id(3, 0)]);
        let nether = LevelChunk::new(Dimension::Nether, position);
        assert_eq!(filed.ids_of(nether), [id(3, 1)]);
        assert_eq!(
            ids(&filed.held(|chunk| chunk == nether).arrivals),
            [id(3, 1)]
        );
        // The stream names the position alone, of the holder's dimension.
        let Some(Handed::Arrival(arrival)) = filed.filed(id(3, 1)) else {
            panic!("an arrival is filed");
        };
        assert_eq!(arrival.chunk, position);
        assert_eq!(
            filed
                .all()
                .iter()
                .map(|(_, dimension)| *dimension)
                .collect::<Vec<_>>(),
            [Dimension::Overworld, Dimension::Nether]
        );
    }

    #[test]
    fn the_arrivals_of_every_dimension_go_forward_each_with_its_dimension() {
        let mut filed = FiledArrivals::default();
        filed.file(id(3, 0), kept(1, 16.5, 4));
        filed.file(id(3, 1), kept_in(Dimension::End, 2, 0.5, 4));
        filed.file(id(5, 0), kept(3, 0.5, 6));
        let forward: Vec<(Dimension, EffectId)> = filed
            .living_in(..=6)
            .arrivals
            .iter()
            .map(|(dimension, arrival)| (*dimension, arrival.id))
            .collect();
        assert_eq!(
            forward,
            [
                (Dimension::Overworld, id(3, 0)),
                (Dimension::End, id(3, 1)),
                (Dimension::Overworld, id(5, 0))
            ]
        );
        filed.remove(id(3, 1));
        assert!(filed.lives_in(4));
    }

    #[test]
    fn a_traveller_is_filed_for_its_chunk_due_at_once_and_handed_as_a_traveller() {
        let mut filed = FiledArrivals::default();
        filed.file(id(3, 0), kept(1, 16.5, 4));
        let sent = traveller(2, 40);
        filed.file(id(3, 1), KeptArrival::traveller(sent.clone(), 4));
        let chunk = sent.chunk;
        assert_eq!(chunk.dimension, Dimension::Nether);
        assert_eq!(filed.ids_of(chunk), [id(3, 1)]);
        assert_eq!(
            filed.get(id(3, 1)).map(|kept| kept.due),
            Some(Due::FromTaking)
        );
        assert_eq!(
            filed.filed(id(3, 1)),
            Some(Handed::Traveller(id(3, 1), sent.clone()))
        );
        // Held and gathered apart from the arrivals, in the order of their numbers.
        let held = filed.held(|_| true);
        assert_eq!(ids(&held.arrivals), [id(3, 0)]);
        assert_eq!(held.travellers, [(id(3, 1), sent.clone())]);
        // Written forward whatever its dimension, never folded with the arrivals.
        let forward = filed.living_in(..=4);
        assert_eq!(forward.travellers, [(id(3, 1), sent.clone())]);
        assert_eq!(forward.arrivals.len(), 1);
        assert_eq!(forward.arrivals[0].1.id, id(3, 0));
        assert_eq!(filed.all().len(), 1);
        assert_eq!(filed.travellers(), [(id(3, 1), sent)]);
        // Taken as an arrival is.
        assert!(filed.remove(id(3, 1)).is_some());
        assert!(!filed.has(chunk));
        assert!(filed.travellers().is_empty());
    }
}
