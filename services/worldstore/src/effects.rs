//! Which region uses which chunk, and the effects filed for those chunks and not taken
//! yet (`docs/adr/0025-effects-between-regions.md`, sections 2 and 5). Both are the
//! commit thread's, which changes them in the order the messages arrive.
//!
//! An effect lives in the segment of the log that has its latest record: the commit
//! that made it, or the record that wrote it forward. A segment is not removed while an
//! effect lives in it; the commit thread writes such effects forward first.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeBounds;

use clustine_region::RegionId;
use clustine_world::{ChunkPos, Due, Effect, EffectId, FiledEffect};

/// The sequence of the record at `offset` in `segment`: its place in the log, which
/// increases for the life of the world. A segment is never appended to beyond
/// `SEGMENT_LIMIT` and one record, far below four gigabytes.
pub(crate) fn sequence(segment: u64, offset: u64) -> u64 {
    debug_assert!(
        offset < 1 << 32,
        "an offset within a segment fits in 32 bits"
    );
    segment << 32 | offset
}

/// Which region uses which chunk. Only the holder of a chunk uses it, so a chunk has
/// one user at most.
#[derive(Default)]
pub(crate) struct Uses {
    user: BTreeMap<ChunkPos, RegionId>,
    chunks: BTreeMap<RegionId, BTreeSet<ChunkPos>>,
}

impl Uses {
    /// `region` uses `chunk` from now on. Whether its user changed.
    pub(crate) fn begin(&mut self, region: RegionId, chunk: ChunkPos) -> bool {
        let before = self.user.insert(chunk, region);
        if let Some(before) = before
            && before != region
            && let Some(chunks) = self.chunks.get_mut(&before)
        {
            chunks.remove(&chunk);
        }
        self.chunks.entry(region).or_default().insert(chunk);
        before != Some(region)
    }

    /// `region` does not use `chunk` any more. Whether it did.
    pub(crate) fn end(&mut self, region: RegionId, chunk: ChunkPos) -> bool {
        if self.user.get(&chunk) != Some(&region) {
            return false;
        }
        self.user.remove(&chunk);
        if let Some(chunks) = self.chunks.get_mut(&region) {
            chunks.remove(&chunk);
            if chunks.is_empty() {
                self.chunks.remove(&region);
            }
        }
        true
    }

    pub(crate) fn user(&self, chunk: ChunkPos) -> Option<RegionId> {
        self.user.get(&chunk).copied()
    }

    /// The chunks `region` uses, in ascending order.
    pub(crate) fn of(&self, region: RegionId) -> Vec<ChunkPos> {
        self.chunks
            .get(&region)
            .map_or_else(Vec::new, |chunks| chunks.iter().copied().collect())
    }

    /// The use of those of `chunks` that `from` uses moves to `to`; all of them where
    /// `chunks` is `None`. Returns the chunks whose use moved, in ascending order.
    pub(crate) fn moved(
        &mut self,
        from: RegionId,
        to: RegionId,
        chunks: Option<&[ChunkPos]>,
    ) -> Vec<ChunkPos> {
        let moved: Vec<ChunkPos> = match chunks {
            None => self.of(from),
            Some(chunks) => {
                let mut moved: Vec<ChunkPos> = chunks
                    .iter()
                    .copied()
                    .filter(|chunk| self.user(*chunk) == Some(from))
                    .collect();
                moved.sort_unstable();
                moved.dedup();
                moved
            }
        };
        for chunk in &moved {
            self.begin(to, *chunk);
        }
        moved
    }
}

/// An effect not taken yet, and the segment its latest record is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Kept {
    pub(crate) effect: Effect,
    pub(crate) due: Due,
    pub(crate) segment: u64,
}

/// The effects filed and not taken or dropped yet.
#[derive(Default)]
pub(crate) struct Filed {
    effects: BTreeMap<EffectId, Kept>,
    by_chunk: BTreeMap<ChunkPos, BTreeSet<EffectId>>,
    /// How many effects live in each segment.
    living: BTreeMap<u64, usize>,
}

impl Filed {
    /// Files an effect, or moves it to the segment of a later record of it.
    pub(crate) fn file(&mut self, id: EffectId, kept: Kept) {
        self.remove(id);
        self.by_chunk
            .entry(kept.effect.at.chunk())
            .or_default()
            .insert(id);
        *self.living.entry(kept.segment).or_default() += 1;
        self.effects.insert(id, kept);
    }

    /// Takes the effect out, if it is filed.
    pub(crate) fn remove(&mut self, id: EffectId) -> Option<Kept> {
        let kept = self.effects.remove(&id)?;
        let chunk = kept.effect.at.chunk();
        if let Some(ids) = self.by_chunk.get_mut(&chunk) {
            ids.remove(&id);
            if ids.is_empty() {
                self.by_chunk.remove(&chunk);
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

    pub(crate) fn get(&self, id: EffectId) -> Option<&Kept> {
        self.effects.get(&id)
    }

    /// The numbers of the effects filed for `chunk`, ascending.
    pub(crate) fn ids_of(&self, chunk: ChunkPos) -> Vec<EffectId> {
        self.by_chunk
            .get(&chunk)
            .map_or_else(Vec::new, |ids| ids.iter().copied().collect())
    }

    /// The effects filed for any of `chunks`, as the stream carries them, ascending by
    /// number.
    pub(crate) fn for_chunks(&self, chunks: &[ChunkPos]) -> Vec<FiledEffect> {
        let mut ids: Vec<EffectId> = chunks
            .iter()
            .flat_map(|chunk| self.ids_of(*chunk))
            .collect();
        ids.sort_unstable();
        ids.into_iter().filter_map(|id| self.filed(id)).collect()
    }

    /// The effect as the stream carries it, if it is filed.
    pub(crate) fn filed(&self, id: EffectId) -> Option<FiledEffect> {
        let kept = self.effects.get(&id)?;
        Some(FiledEffect {
            id,
            effect: kept.effect,
            due: kept.due,
        })
    }

    /// A scheduled tick filed for one of `chunks` counts its delay from its taking from
    /// now on: the use of the chunks moved to another region, whose ticks are not those
    /// it was converted to (the record, "Found while building").
    pub(crate) fn count_from_taking(&mut self, chunks: &[ChunkPos]) {
        for chunk in chunks {
            for id in self.by_chunk.get(chunk).into_iter().flatten() {
                if let Some(kept) = self.effects.get_mut(id) {
                    kept.due = Due::FromTaking;
                }
            }
        }
    }

    /// Every scheduled tick filed counts its delay from its taking from now on. For a
    /// store that reads its log at a start, which does not know who used a chunk when a
    /// merge or a split was written: a tick is then late by its delay at most.
    pub(crate) fn all_count_from_taking(&mut self) {
        for kept in self.effects.values_mut() {
            kept.due = Due::FromTaking;
        }
    }

    /// Whether an effect lives in `segment`.
    pub(crate) fn lives_in(&self, segment: u64) -> bool {
        self.living.contains_key(&segment)
    }

    /// The effects that live in the segments of `segments`, ascending by number.
    pub(crate) fn living_in(&self, segments: impl RangeBounds<u64>) -> Vec<FiledEffect> {
        let segments: BTreeSet<u64> = self.living.range(segments).map(|(s, _)| *s).collect();
        if segments.is_empty() {
            return Vec::new();
        }
        self.effects
            .iter()
            .filter(|(_, kept)| segments.contains(&kept.segment))
            .filter_map(|(id, _)| self.filed(*id))
            .collect()
    }

    /// Every effect filed, ascending by number.
    pub(crate) fn all(&self) -> Vec<FiledEffect> {
        self.effects
            .keys()
            .filter_map(|id| self.filed(*id))
            .collect()
    }
}

/// Something a group did to the effects filed, which is taken back, last first, if the
/// group cannot be made durable.
pub(crate) enum Undo {
    Filed(EffectId),
    Removed(EffectId, Kept),
}

impl Filed {
    pub(crate) fn undo(&mut self, undo: Vec<Undo>) {
        for step in undo.into_iter().rev() {
            match step {
                Undo::Filed(id) => {
                    self.remove(id);
                }
                Undo::Removed(id, kept) => self.file(id, kept),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_world::{BlockPos, Effected};

    use super::*;

    fn effect(x: i32) -> Effect {
        Effect {
            at: BlockPos::new(x, 64, 0),
            what: Effected::SetIf {
                expected: blocks::AIR,
                state: blocks::FIRE,
                flags: 3,
            },
        }
    }

    fn kept(x: i32, segment: u64) -> Kept {
        Kept {
            effect: effect(x),
            due: Due::At { tick: 9 },
            segment,
        }
    }

    fn id(sequence: u64, index: u32) -> EffectId {
        EffectId { sequence, index }
    }

    #[test]
    fn sequences_increase_with_the_segment_and_then_the_offset() {
        assert!(sequence(1, 9_000_000) < sequence(2, 0));
        assert!(sequence(2, 0) < sequence(2, 1));
    }

    #[test]
    fn a_chunk_has_one_user_and_its_use_moves_whole_or_in_part() {
        let mut uses = Uses::default();
        let (a, b, c) = (RegionId(1), RegionId(2), RegionId(3));
        let chunks: Vec<ChunkPos> = (0..4).map(|x| ChunkPos::new(x, 0)).collect();
        for chunk in &chunks {
            uses.begin(a, *chunk);
        }
        uses.begin(b, chunks[3]);
        assert_eq!(uses.of(a), chunks[..3]);
        assert_eq!(uses.user(chunks[3]), Some(b));
        assert!(!uses.end(a, chunks[3]));
        assert_eq!(
            uses.moved(a, c, Some(&[chunks[2], chunks[3], chunks[2]])),
            [chunks[2]]
        );
        assert_eq!(uses.moved(a, b, None), chunks[..2]);
        assert_eq!(uses.of(a), []);
        assert_eq!(uses.of(b), [chunks[0], chunks[1], chunks[3]]);
        assert!(uses.end(c, chunks[2]));
        assert_eq!(uses.user(chunks[2]), None);
    }

    #[test]
    fn an_effect_lives_in_the_segment_of_its_latest_record() {
        let mut filed = Filed::default();
        filed.file(id(1, 0), kept(0, 1));
        filed.file(id(1, 1), kept(20, 1));
        filed.file(id(2, 0), kept(1, 2));
        assert!(filed.lives_in(1));
        assert_eq!(
            filed
                .living_in(..=1)
                .iter()
                .map(|f| f.id)
                .collect::<Vec<_>>(),
            [id(1, 0), id(1, 1)]
        );
        // Written forward into segment 3.
        filed.file(id(1, 0), kept(0, 3));
        filed.file(id(1, 1), kept(20, 3));
        assert!(!filed.lives_in(1));
        assert_eq!(filed.ids_of(ChunkPos::new(0, 0)), [id(1, 0), id(2, 0)]);
        assert_eq!(
            filed
                .for_chunks(&[ChunkPos::new(1, 0), ChunkPos::new(0, 0)])
                .iter()
                .map(|f| f.id)
                .collect::<Vec<_>>(),
            [id(1, 0), id(1, 1), id(2, 0)]
        );
    }

    #[test]
    fn what_a_group_did_is_taken_back_last_first() {
        let mut filed = Filed::default();
        filed.file(id(1, 0), kept(0, 1));
        let before = filed.all();
        let mut undo = Vec::new();
        filed.file(id(2, 0), kept(1, 2));
        undo.push(Undo::Filed(id(2, 0)));
        let taken = filed.remove(id(1, 0)).unwrap();
        undo.push(Undo::Removed(id(1, 0), taken));
        let taken = filed.remove(id(2, 0)).unwrap();
        undo.push(Undo::Removed(id(2, 0), taken));
        filed.undo(undo);
        assert_eq!(filed.all(), before);
        assert!(!filed.lives_in(2));
    }

    #[test]
    fn a_tick_whose_chunk_changes_hands_counts_from_its_taking() {
        let mut filed = Filed::default();
        filed.file(id(1, 0), kept(0, 1));
        filed.file(id(1, 1), kept(20, 1));
        filed.count_from_taking(&[ChunkPos::new(1, 0)]);
        assert_eq!(filed.get(id(1, 0)).unwrap().due, Due::At { tick: 9 });
        assert_eq!(filed.get(id(1, 1)).unwrap().due, Due::FromTaking);
    }
}
