//! Who keeps a border copy of which chunk (`docs/adr/0025-effects-between-regions.md`,
//! section 3), the commit thread's alone.
//!
//! A copy is kept up to date by the block changes of each later commit in its chunk,
//! each with the sequence of the change before, so that a gap shows. The chain of a
//! chunk is anchored where it was first copied: its base says that sequence as its
//! last change, and the first update names it as the one before. A region that copies
//! a chunk already copied by another is anchored where the chain is.

use std::collections::{BTreeMap, BTreeSet};

use clustine_region::RegionId;
use clustine_world::ChunkPos;

#[derive(Default)]
pub(crate) struct Copies {
    copiers: BTreeMap<ChunkPos, BTreeSet<RegionId>>,
    copied: BTreeMap<RegionId, BTreeSet<ChunkPos>>,
    /// Of every chunk copied, the sequence of the last change its copiers were told of,
    /// or of the group in which it was first copied.
    anchors: BTreeMap<ChunkPos, u64>,
}

impl Copies {
    /// `region` copies `chunk` from the group of `sequence` on. Returns the last change
    /// its base is to name.
    pub(crate) fn register(&mut self, region: RegionId, chunk: ChunkPos, sequence: u64) -> u64 {
        self.copiers.entry(chunk).or_default().insert(region);
        self.copied.entry(region).or_default().insert(chunk);
        *self.anchors.entry(chunk).or_insert(sequence)
    }

    /// `region` copies `chunk` no longer.
    pub(crate) fn unregister(&mut self, region: RegionId, chunk: ChunkPos) {
        if let Some(copiers) = self.copiers.get_mut(&chunk) {
            copiers.remove(&region);
            if copiers.is_empty() {
                self.copiers.remove(&chunk);
                self.anchors.remove(&chunk);
            }
        }
        if let Some(copied) = self.copied.get_mut(&region) {
            copied.remove(&chunk);
            if copied.is_empty() {
                self.copied.remove(&region);
            }
        }
    }

    /// `region` copies nothing any more.
    pub(crate) fn forget(&mut self, region: RegionId) {
        for chunk in self.copied.remove(&region).unwrap_or_default() {
            if let Some(copiers) = self.copiers.get_mut(&chunk) {
                copiers.remove(&region);
                if copiers.is_empty() {
                    self.copiers.remove(&chunk);
                    self.anchors.remove(&chunk);
                }
            }
        }
    }

    /// The regions that copy `chunk`, ascending.
    pub(crate) fn copiers(&self, chunk: ChunkPos) -> Vec<RegionId> {
        self.copiers
            .get(&chunk)
            .map_or_else(Vec::new, |copiers| copiers.iter().copied().collect())
    }

    /// Whether `region` copies `chunk`.
    pub(crate) fn copies(&self, region: RegionId, chunk: ChunkPos) -> bool {
        self.copiers
            .get(&chunk)
            .is_some_and(|copiers| copiers.contains(&region))
    }

    /// `chunk` changed in the record of `sequence`: the change before, which its update
    /// names, if anyone copies it.
    pub(crate) fn changed(&mut self, chunk: ChunkPos, sequence: u64) -> Option<u64> {
        let anchor = self.anchors.get_mut(&chunk)?;
        Some(std::mem::replace(anchor, sequence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_is_anchored_where_its_chunk_was_first_copied() {
        let mut copies = Copies::default();
        let chunk = ChunkPos::new(3, 4);
        let (a, b) = (RegionId(1), RegionId(2));
        assert_eq!(copies.changed(chunk, 5), None);
        assert_eq!(copies.register(a, chunk, 10), 10);
        assert_eq!(copies.changed(chunk, 12), Some(10));
        // Copied by another later: its base names the chain's last change.
        assert_eq!(copies.register(b, chunk, 20), 12);
        assert_eq!(copies.changed(chunk, 21), Some(12));
        assert_eq!(copies.copiers(chunk), [a, b]);
        copies.unregister(a, chunk);
        assert!(!copies.copies(a, chunk));
        copies.forget(b);
        assert_eq!(copies.copiers(chunk), []);
        // Nobody copies it: the next copier begins a chain of its own.
        assert_eq!(copies.register(a, chunk, 30), 30);
    }
}
