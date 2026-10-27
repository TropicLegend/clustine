//! The regions' points of interest, and the margin each region is published of the others'
//! (`docs/groundwork/n10-plan.md`, section 3.5). In memory only, never in the log: a store
//! started again knows nothing of a chunk until a region holding it reports it, which its
//! first tick after it is opened does for all of its land.
//!
//! A region reads points of interest up to five chunks beyond its land (a raid's centre is
//! the mean of those within 64 blocks, and the village reaches five sections). It holds and
//! copies some of those chunks; of the others it is published, once a game second, what
//! their holders last reported: each record's place, type and whether it is occupied.
//!
//! A region's word is taken only from its current owner: a partial report of another
//! session than the one whose full report gave the region's land is let go. A region's full report (its first tick after an opening: a start, a
//! take-over, a merge, a split, a move) replaces its land, and makes its next publication
//! whole, so that a region that began anew learns its margin again. A chunk a region lets
//! go keeps the records it last reported until another region reports it: the store has
//! nothing better to say of a chunk returned to nobody.
//!
//! Publications are not held back for other regions' reports, as the counts are
//! (`spawn_counts.rs`): a chunk's records are its holder's word alone, and a region that is
//! being opened again keeps what it last reported meanwhile.

use std::collections::{BTreeMap, BTreeSet};

use clustine_region::RegionId;
use clustine_rpc::{PoiSeen, PublishedVillage, VillageReport};
use clustine_world::ChunkPos;

/// How far beyond a region's land, in chunks, it is published points of interest.
const MARGIN: i32 = 5;

/// The chunks a region's owner of `session` holds, by its reports.
#[derive(Debug, Default)]
struct Land {
    session: u64,
    chunks: BTreeSet<ChunkPos>,
}

/// What a region's owner of `session` was last published.
#[derive(Debug)]
struct Published {
    session: u64,
    chunks: BTreeMap<ChunkPos, Vec<PoiSeen>>,
}

#[derive(Debug, Default)]
pub(crate) struct Village {
    /// Each chunk's records, as the region that held it last reported them.
    chunks: BTreeMap<ChunkPos, Vec<PoiSeen>>,
    lands: BTreeMap<RegionId, Land>,
    published: BTreeMap<RegionId, Published>,
}

impl Village {
    /// `report` of the region `region`'s owner of the session `session`. A full report
    /// replaces the region's land and makes its next publication whole; a partial one of
    /// another owner than the land's is let go (its full report comes first).
    pub(crate) fn report(&mut self, region: RegionId, session: u64, report: VillageReport) {
        if report.full {
            let chunks = report.pois.iter().map(|(position, _)| *position).collect();
            self.lands.insert(region, Land { session, chunks });
            self.published.remove(&region);
        } else if self
            .lands
            .get(&region)
            .is_none_or(|land| land.session != session)
        {
            return;
        }
        let land = self.lands.get_mut(&region).expect("kept above");
        for position in report.let_go {
            land.chunks.remove(&position);
        }
        for (position, pois) in report.pois {
            land.chunks.insert(position);
            self.chunks.insert(position, pois);
        }
    }

    /// What `region`, owned by `session`, is to be published now, if anything: the
    /// records of the chunks within [`MARGIN`] of its land that it neither holds nor
    /// copies (`copies` says whether it copies a chunk), whole at the first publication to
    /// the session, else the changes since the last. None before the region's first full
    /// report, and none where nothing changed.
    pub(crate) fn publication(
        &mut self,
        region: RegionId,
        session: u64,
        copies: impl Fn(ChunkPos) -> bool,
    ) -> Option<PublishedVillage> {
        let land = self
            .lands
            .get(&region)
            .filter(|land| land.session == session)?;
        let near_land = |position: ChunkPos| {
            (-MARGIN..=MARGIN).any(|dz| {
                (-MARGIN..=MARGIN).any(|dx| {
                    land.chunks
                        .contains(&ChunkPos::new(position.x + dx, position.z + dz))
                })
            })
        };
        let wanted: BTreeMap<ChunkPos, Vec<PoiSeen>> = self
            .chunks
            .iter()
            .filter(|(position, pois)| {
                !pois.is_empty()
                    && !land.chunks.contains(position)
                    && !copies(**position)
                    && near_land(**position)
            })
            .map(|(position, pois)| (*position, pois.clone()))
            .collect();
        let earlier = self
            .published
            .get(&region)
            .filter(|published| published.session == session);
        let publication = match earlier {
            None => PublishedVillage {
                whole: true,
                pois: wanted.iter().map(|(p, seen)| (*p, seen.clone())).collect(),
                dropped: Vec::new(),
            },
            Some(earlier) => {
                let pois: Vec<(ChunkPos, Vec<PoiSeen>)> = wanted
                    .iter()
                    .filter(|(position, seen)| earlier.chunks.get(position) != Some(seen))
                    .map(|(position, seen)| (*position, seen.clone()))
                    .collect();
                let dropped: Vec<ChunkPos> = earlier
                    .chunks
                    .keys()
                    .filter(|position| !wanted.contains_key(position))
                    .copied()
                    .collect();
                if pois.is_empty() && dropped.is_empty() {
                    return None;
                }
                PublishedVillage {
                    whole: false,
                    pois,
                    dropped,
                }
            }
        };
        self.published.insert(
            region,
            Published {
                session,
                chunks: wanted,
            },
        );
        Some(publication)
    }
}

#[cfg(test)]
mod tests {
    use clustine_world::BlockPos;
    use clustine_world::poi::PoiType;

    use super::*;

    const WEST: RegionId = RegionId(0);
    const EAST: RegionId = RegionId(1);

    fn bed(x: i32, occupied: bool) -> PoiSeen {
        PoiSeen {
            at: BlockPos::new(x, 64, 0),
            kind: PoiType::Home,
            occupied,
        }
    }

    fn full(chunks: &[(i32, Vec<PoiSeen>)]) -> VillageReport {
        VillageReport {
            full: true,
            pois: chunks
                .iter()
                .map(|(x, seen)| (ChunkPos::new(*x, 0), seen.clone()))
                .collect(),
            let_go: Vec::new(),
        }
    }

    fn nothing_copied(_: ChunkPos) -> bool {
        false
    }

    /// West holds chunks x = -3 to -1, east x = 0 to 2; a bed in east's chunk 1.
    fn two_regions() -> Village {
        let mut village = Village::default();
        village.report(
            WEST,
            1,
            full(&[(-3, Vec::new()), (-2, Vec::new()), (-1, Vec::new())]),
        );
        village.report(
            EAST,
            1,
            full(&[(0, Vec::new()), (1, vec![bed(20, true)]), (2, Vec::new())]),
        );
        village
    }

    #[test]
    fn a_region_is_published_whole_first_then_only_what_changed() {
        let mut village = two_regions();
        let first = village.publication(WEST, 1, nothing_copied).unwrap();
        assert!(first.whole);
        assert_eq!(first.pois, [(ChunkPos::new(1, 0), vec![bed(20, true)])]);
        assert_eq!(village.publication(WEST, 1, nothing_copied), None);

        village.report(
            EAST,
            1,
            VillageReport {
                full: false,
                pois: vec![(ChunkPos::new(1, 0), vec![bed(20, false)])],
                let_go: Vec::new(),
            },
        );
        let second = village.publication(WEST, 1, nothing_copied).unwrap();
        assert!(!second.whole);
        assert_eq!(second.pois, [(ChunkPos::new(1, 0), vec![bed(20, false)])]);

        village.report(
            EAST,
            1,
            VillageReport {
                full: false,
                pois: vec![(ChunkPos::new(1, 0), Vec::new())],
                let_go: Vec::new(),
            },
        );
        let third = village.publication(WEST, 1, nothing_copied).unwrap();
        assert_eq!(third.dropped, [ChunkPos::new(1, 0)]);
        // A region is never published its own land.
        assert_eq!(
            village.publication(EAST, 1, nothing_copied).unwrap().pois,
            []
        );
    }

    #[test]
    fn a_copied_chunk_and_one_beyond_five_chunks_are_not_published() {
        let mut village = two_regions();
        let copied = |position: ChunkPos| position == ChunkPos::new(1, 0);
        assert_eq!(village.publication(WEST, 1, copied).unwrap().pois, []);
        let mut far = Village::default();
        far.report(WEST, 1, full(&[(-5, Vec::new())]));
        far.report(EAST, 1, full(&[(1, vec![bed(20, true)])]));
        assert_eq!(far.publication(WEST, 1, nothing_copied).unwrap().pois, []);
    }

    #[test]
    fn a_new_owner_s_full_report_makes_the_next_publication_whole_and_an_old_one_is_let_go() {
        let mut village = two_regions();
        village.publication(WEST, 1, nothing_copied).unwrap();
        // The region opened again by another owner: until its full report, nothing.
        assert_eq!(village.publication(WEST, 2, nothing_copied), None);
        village.report(WEST, 2, full(&[(-1, Vec::new())]));
        assert!(village.publication(WEST, 2, nothing_copied).unwrap().whole);
        // A partial report of the owner before is let go.
        village.report(
            WEST,
            1,
            VillageReport {
                full: false,
                pois: vec![(ChunkPos::new(-4, 0), vec![bed(-60, true)])],
                let_go: Vec::new(),
            },
        );
        assert_eq!(
            village.publication(EAST, 1, nothing_copied).unwrap().pois,
            []
        );
    }

    /// A merge or a split restores a region within its owner's session: its first tick
    /// reports its land whole, and its margin is published whole again, since the region
    /// began anew without it.
    #[test]
    fn a_full_report_of_the_same_owner_makes_the_next_publication_whole() {
        let mut village = two_regions();
        village.publication(WEST, 1, nothing_copied).unwrap();
        assert_eq!(village.publication(WEST, 1, nothing_copied), None);
        village.report(
            WEST,
            1,
            full(&[(-3, Vec::new()), (-2, Vec::new()), (-1, Vec::new())]),
        );
        let again = village.publication(WEST, 1, nothing_copied).unwrap();
        assert!(again.whole);
        assert_eq!(again.pois, [(ChunkPos::new(1, 0), vec![bed(20, true)])]);
    }

    #[test]
    fn a_chunk_let_go_keeps_its_records_until_another_region_reports_it() {
        let mut village = two_regions();
        village.report(
            EAST,
            1,
            VillageReport {
                full: false,
                pois: Vec::new(),
                let_go: vec![ChunkPos::new(1, 0)],
            },
        );
        let published = village.publication(WEST, 1, nothing_copied).unwrap();
        assert_eq!(published.pois, [(ChunkPos::new(1, 0), vec![bed(20, true)])]);
        // West takes it: it is its land now, and not published to it.
        village.report(
            WEST,
            1,
            VillageReport {
                full: false,
                pois: vec![(ChunkPos::new(1, 0), vec![bed(20, true)])],
                let_go: Vec::new(),
            },
        );
        let after = village.publication(WEST, 1, nothing_copied).unwrap();
        assert_eq!(after.dropped, [ChunkPos::new(1, 0)]);
    }
}
