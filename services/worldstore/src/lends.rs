//! The lends under way (`docs/adr/0027-lending-at-a-line.md`, section 3): asked of a
//! lender and not yet granted, refused or called off. Kept in memory only: a lend under
//! way is called off at any hello, merge or split of either region, and a store that
//! starts has none, as nobody asked it for one.

use std::collections::BTreeMap;

use clustine_region::RegionId;
use clustine_world::LevelChunk;

/// How many chunk rows of z a stretch of a line has, within which lends between two
/// regions go one way at a time (the same record, section 1).
pub(crate) const STRETCH_ROWS: i32 = 8;

/// The stretch of a line that `chunk` is in.
pub(crate) fn stretch(chunk: LevelChunk) -> i32 {
    chunk.position.z.div_euclid(STRETCH_ROWS)
}

/// A lend under way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lend {
    pub(crate) lender: RegionId,
    pub(crate) borrower: RegionId,
    /// Its chunks, each with the number of the lender's return that gives it back for
    /// the lend, once the lender has said `Lending` of it.
    pub(crate) chunks: BTreeMap<LevelChunk, Option<u64>>,
}

#[derive(Debug, Default)]
pub(crate) struct Lends {
    /// The number the next lend gets. Never given twice in a store's life, so that an
    /// answer to a lend that was called off is not taken for a later one's.
    next: u64,
    under_way: BTreeMap<u64, Lend>,
    /// The lend each chunk is under.
    by_chunk: BTreeMap<LevelChunk, u64>,
}

impl Lends {
    /// A lend of `chunks`, none of which is under lend, from `lender` to `borrower`.
    /// Returns its number.
    pub(crate) fn begin(
        &mut self,
        lender: RegionId,
        borrower: RegionId,
        chunks: &[LevelChunk],
    ) -> u64 {
        self.next += 1;
        let lend = self.next;
        for chunk in chunks {
            let before = self.by_chunk.insert(*chunk, lend);
            debug_assert!(before.is_none(), "a chunk is under one lend at most");
        }
        let lend_of = Lend {
            lender,
            borrower,
            chunks: chunks.iter().map(|chunk| (*chunk, None)).collect(),
        };
        self.under_way.insert(lend, lend_of);
        lend
    }

    /// The lend `chunk` is under, if any.
    pub(crate) fn of_chunk(&self, chunk: LevelChunk) -> Option<u64> {
        self.by_chunk.get(&chunk).copied()
    }

    pub(crate) fn get(&self, lend: u64) -> Option<&Lend> {
        self.under_way.get(&lend)
    }

    /// Notes that the lender gives `chunk` back for `lend` with its return `number`.
    pub(crate) fn returning(&mut self, lend: u64, chunk: LevelChunk, number: u64) {
        if let Some(of) = self
            .under_way
            .get_mut(&lend)
            .and_then(|lend| lend.chunks.get_mut(&chunk))
        {
            *of = Some(number);
        }
    }

    /// Takes those of `chunks` that are under `lend` out of it, which ends the lend once
    /// it has none left. Returns those taken, in the order of `chunks`.
    pub(crate) fn take(&mut self, lend: u64, chunks: &[LevelChunk]) -> Vec<LevelChunk> {
        let Some(of) = self.under_way.get_mut(&lend) else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        for chunk in chunks {
            if of.chunks.remove(chunk).is_some() {
                self.by_chunk.remove(chunk);
                taken.push(*chunk);
            }
        }
        if of.chunks.is_empty() {
            self.under_way.remove(&lend);
        }
        taken
    }

    /// Ends every lend `region` lends or borrows in, and returns them with their numbers.
    pub(crate) fn call_off(&mut self, region: RegionId) -> Vec<(u64, Lend)> {
        let ended: Vec<u64> = self
            .under_way
            .iter()
            .filter(|(_, lend)| lend.lender == region || lend.borrower == region)
            .map(|(number, _)| *number)
            .collect();
        ended
            .into_iter()
            .filter_map(|number| {
                let lend = self.under_way.remove(&number)?;
                for chunk in lend.chunks.keys() {
                    self.by_chunk.remove(chunk);
                }
                Some((number, lend))
            })
            .collect()
    }

    /// Ends every lend, as when the log fails and every region loses its owner.
    pub(crate) fn clear(&mut self) {
        self.under_way.clear();
        self.by_chunk.clear();
    }

    /// Whether a lend from `lender` to `borrower` is under way in the stretch `stretch`.
    pub(crate) fn under_way_in(&self, lender: RegionId, borrower: RegionId, stretch: i32) -> bool {
        self.under_way.values().any(|lend| {
            lend.lender == lender
                && lend.borrower == borrower
                && lend
                    .chunks
                    .keys()
                    .any(|chunk| self::stretch(*chunk) == stretch)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overworld's chunk at `x`, `z`, where lends are: pins are the overworld's.
    fn at(x: i32, z: i32) -> LevelChunk {
        LevelChunk::new(
            clustine_world::Dimension::Overworld,
            clustine_world::ChunkPos::new(x, z),
        )
    }

    const LENDER: RegionId = RegionId(0);
    const BORROWER: RegionId = RegionId(1);

    #[test]
    fn a_lend_ends_when_its_last_chunk_is_taken_and_its_number_is_not_given_again() {
        let mut lends = Lends::default();
        let one = at(0, 0);
        let two = at(0, 1);
        let lend = lends.begin(LENDER, BORROWER, &[one, two]);
        assert_eq!(lends.of_chunk(two), Some(lend));
        assert_eq!(lends.take(lend, &[one, at(5, 5)]), [one]);
        assert!(lends.get(lend).is_some());
        assert_eq!(lends.take(lend, &[two]), [two]);
        assert!(lends.get(lend).is_none());
        assert_eq!(lends.of_chunk(two), None);
        assert_ne!(lends.begin(LENDER, BORROWER, &[two]), lend);
    }

    #[test]
    fn calling_off_a_region_ends_the_lends_it_lends_or_borrows_in_only() {
        let mut lends = Lends::default();
        let third = RegionId(2);
        let first = lends.begin(LENDER, BORROWER, &[at(0, 0)]);
        let second = lends.begin(third, LENDER, &[at(9, 0)]);
        let other = lends.begin(third, BORROWER, &[at(9, 9)]);
        let ended: Vec<u64> = lends
            .call_off(LENDER)
            .into_iter()
            .map(|(number, _)| number)
            .collect();
        assert_eq!(ended, [first, second]);
        assert!(lends.get(other).is_some());
        assert_eq!(lends.of_chunk(at(0, 0)), None);
    }

    #[test]
    fn a_stretch_is_eight_rows_of_z_rounded_down() {
        assert_eq!(stretch(at(3, 7)), 0);
        assert_eq!(stretch(at(3, 8)), 1);
        assert_eq!(stretch(at(3, -1)), -1);
        assert_eq!(stretch(at(3, -8)), -1);
        assert_eq!(stretch(at(3, -9)), -2);
        let mut lends = Lends::default();
        lends.begin(LENDER, BORROWER, &[at(0, 9)]);
        assert!(lends.under_way_in(LENDER, BORROWER, 1));
        assert!(!lends.under_way_in(BORROWER, LENDER, 1));
        assert!(!lends.under_way_in(LENDER, BORROWER, 0));
    }
}
