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
//!
//! A chunk is named by its dimension and its position, and a region is published what
//! the regions of its own dimension reported: the game keeps points of interest and
//! raids by level (`docs/groundwork/p-plan.md`, section 3.3).

use std::collections::{BTreeMap, BTreeSet};

use clustine_region::RegionId;
use clustine_rpc::{
    FightSeen, GlobalLevelEvent, PoiSeen, PublishedVillage, RaidSeen, VillageReport,
};
use clustine_world::{ChunkPos, Dimension, LevelChunk};

/// How far beyond a region's land, in chunks, it is published points of interest.
const MARGIN: i32 = 5;

/// How many of the latest global level events the store keeps to publish: a region's
/// owner published less often than that many are made misses the oldest.
const GLOBAL_EVENTS_KEPT: usize = 256;

/// The chunks a region's owner of `session` holds, by its reports.
#[derive(Debug, Default)]
struct Land {
    session: u64,
    /// The region's dimension, whose chunks they are.
    dimension: Dimension,
    chunks: BTreeSet<LevelChunk>,
}

/// What a region's owner of `session` was last published.
#[derive(Debug)]
struct Published {
    session: u64,
    chunks: BTreeMap<LevelChunk, Vec<PoiSeen>>,
    raids: Vec<RaidSeen>,
    fight: Option<FightSeen>,
    arena_wanted: bool,
    /// The number of the last global level event published to the session.
    last_global: u64,
}

/// A region's raids as it last reported them, and the order of that report among all,
/// so that of two regions naming one raid (a split's parent until the part reports) the
/// later word holds.
#[derive(Debug)]
struct Raids {
    session: u64,
    /// The region's dimension, whose raids they are.
    dimension: Dimension,
    order: u64,
    raids: Vec<RaidSeen>,
}

#[derive(Debug, Default)]
pub(crate) struct Village {
    /// Each chunk's records, as the region that held it last reported them.
    chunks: BTreeMap<LevelChunk, Vec<PoiSeen>>,
    lands: BTreeMap<RegionId, Land>,
    published: BTreeMap<RegionId, Published>,
    /// Each region's raids (`docs/groundwork/n10-plan.md`, section 9.2).
    raids: BTreeMap<RegionId, Raids>,
    reports: u64,
    /// The dragon fight's bar as a region of the End last said it, with the region and
    /// its owner's session (`docs/groundwork/p-plan.md`, section 19.6).
    fight: Option<(RegionId, u64, FightSeen)>,
    /// Whether each region of the End, by its owner's session, has a player within the
    /// fight's reach.
    in_arena: BTreeMap<RegionId, (u64, bool)>,
    /// The latest global level events, each with its number, oldest first.
    global_events: Vec<(u64, GlobalLevelEvent)>,
    last_global: u64,
}

impl Village {
    /// `report` of the region `region`'s owner of the session `session`, whose chunks
    /// are of `dimension`, the region's. A full report replaces the region's land and
    /// makes its next publication whole; a partial one of another owner than the land's
    /// is let go (its full report comes first).
    pub(crate) fn report(
        &mut self,
        region: RegionId,
        session: u64,
        dimension: Dimension,
        report: VillageReport,
    ) {
        let level = |position| LevelChunk::new(dimension, position);
        if report.full {
            let chunks = report
                .pois
                .iter()
                .map(|(position, _)| level(*position))
                .collect();
            let land = Land {
                session,
                dimension,
                chunks,
            };
            self.lands.insert(region, land);
            self.published.remove(&region);
        } else if self
            .lands
            .get(&region)
            .is_none_or(|land| land.session != session)
        {
            return;
        }
        if let Some(fight) = report.fight {
            self.fight = Some((region, session, fight));
        }
        if let Some(in_arena) = report.in_arena {
            self.in_arena.insert(region, (session, in_arena));
        }
        for event in report.global_events {
            self.last_global += 1;
            self.global_events.push((self.last_global, event));
        }
        let excess = self.global_events.len().saturating_sub(GLOBAL_EVENTS_KEPT);
        self.global_events.drain(..excess);
        if let Some(raids) = report.raids {
            self.reports += 1;
            self.raids.insert(
                region,
                Raids {
                    session,
                    dimension,
                    order: self.reports,
                    raids,
                },
            );
        }
        let land = self.lands.get_mut(&region).expect("kept above");
        for position in report.let_go {
            land.chunks.remove(&level(position));
        }
        for (position, pois) in report.pois {
            land.chunks.insert(level(position));
            self.chunks.insert(level(position), pois);
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
        copies: impl Fn(LevelChunk) -> bool,
        owners: &BTreeMap<RegionId, u64>,
    ) -> Option<PublishedVillage> {
        let land = self
            .lands
            .get(&region)
            .filter(|land| land.session == session)?;
        let dimension = land.dimension;
        let near_land = |chunk: LevelChunk| {
            let position = chunk.position;
            (-MARGIN..=MARGIN).any(|dz| {
                (-MARGIN..=MARGIN).any(|dx| {
                    let near = ChunkPos::new(position.x + dx, position.z + dz);
                    land.chunks.contains(&LevelChunk::new(dimension, near))
                })
            })
        };
        let wanted: BTreeMap<LevelChunk, Vec<PoiSeen>> = self
            .chunks
            .iter()
            .filter(|(chunk, pois)| {
                chunk.dimension == dimension
                    && !pois.is_empty()
                    && !land.chunks.contains(chunk)
                    && !copies(**chunk)
                    && near_land(**chunk)
            })
            .map(|(chunk, pois)| (*chunk, pois.clone()))
            .collect();
        // The other regions' raids of the same dimension, of each region only as its
        // owner now said them (a region absorbed or taken over says nothing), and of one
        // id the later word.
        let mut by_id: BTreeMap<clustine_world::EntityId, (u64, &RaidSeen)> = BTreeMap::new();
        for (holder, kept) in &self.raids {
            if *holder == region
                || owners.get(holder) != Some(&kept.session)
                || kept.dimension != dimension
            {
                continue;
            }
            for raid in &kept.raids {
                let later = by_id
                    .get(&raid.id)
                    .is_none_or(|(order, _)| *order < kept.order);
                if later {
                    by_id.insert(raid.id, (kept.order, raid));
                }
            }
        }
        let raids: Vec<RaidSeen> = by_id.into_values().map(|(_, raid)| raid.clone()).collect();
        // The End's: the fight's bar to its other regions, as its holder's owner now
        // said it; and whether another region of the End has a player in its reach.
        let end = dimension == Dimension::End;
        let fight = self
            .fight
            .filter(|(holder, kept, _)| {
                end && *holder != region && owners.get(holder) == Some(kept)
            })
            .map(|(_, _, fight)| fight);
        let arena_wanted = end
            && self.in_arena.iter().any(|(other, (kept, wanted))| {
                *other != region && *wanted && owners.get(other) == Some(kept)
            });
        let earlier = self
            .published
            .get(&region)
            .filter(|published| published.session == session);
        // A session is told the global events made from its first publication on.
        let since = earlier.map_or(self.last_global, |earlier| earlier.last_global);
        let global_events: Vec<(u64, GlobalLevelEvent)> = self
            .global_events
            .iter()
            .filter(|(number, _)| *number > since)
            .copied()
            .collect();
        let publication = match earlier {
            None => PublishedVillage {
                whole: true,
                pois: wanted
                    .iter()
                    .map(|(chunk, seen)| (chunk.position, seen.clone()))
                    .collect(),
                dropped: Vec::new(),
                raids: raids.clone(),
                fight,
                arena_wanted,
                global_events,
            },
            Some(earlier) => {
                let pois: Vec<(ChunkPos, Vec<PoiSeen>)> = wanted
                    .iter()
                    .filter(|(chunk, seen)| earlier.chunks.get(chunk) != Some(seen))
                    .map(|(chunk, seen)| (chunk.position, seen.clone()))
                    .collect();
                let dropped: Vec<ChunkPos> = earlier
                    .chunks
                    .keys()
                    .filter(|chunk| !wanted.contains_key(chunk))
                    .map(|chunk| chunk.position)
                    .collect();
                if pois.is_empty()
                    && dropped.is_empty()
                    && earlier.raids == raids
                    && earlier.fight == fight
                    && earlier.arena_wanted == arena_wanted
                    && global_events.is_empty()
                {
                    return None;
                }
                PublishedVillage {
                    whole: false,
                    pois,
                    dropped,
                    raids: raids.clone(),
                    fight,
                    arena_wanted,
                    global_events,
                }
            }
        };
        self.published.insert(
            region,
            Published {
                session,
                chunks: wanted,
                raids,
                fight,
                arena_wanted,
                last_global: self.last_global,
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
            fight: None,
            in_arena: None,
            global_events: Vec::new(),
            full: true,
            pois: chunks
                .iter()
                .map(|(x, seen)| (ChunkPos::new(*x, 0), seen.clone()))
                .collect(),
            let_go: Vec::new(),
            raids: None,
        }
    }

    fn nothing_copied(_: LevelChunk) -> bool {
        false
    }

    /// West holds chunks x = -3 to -1, east x = 0 to 2; a bed in east's chunk 1.
    fn two_regions() -> Village {
        let mut village = Village::default();
        village.report(
            WEST,
            1,
            Dimension::Overworld,
            full(&[(-3, Vec::new()), (-2, Vec::new()), (-1, Vec::new())]),
        );
        village.report(
            EAST,
            1,
            Dimension::Overworld,
            full(&[(0, Vec::new()), (1, vec![bed(20, true)]), (2, Vec::new())]),
        );
        village
    }

    /// The End's two regions, a home and another, each of session 1, and an overworld
    /// region.
    fn the_end_and_the_overworld() -> (Village, BTreeMap<RegionId, u64>) {
        let mut village = Village::default();
        village.report(WEST, 1, Dimension::End, full(&[(0, Vec::new())]));
        village.report(EAST, 1, Dimension::End, full(&[(40, Vec::new())]));
        village.report(
            RegionId(2),
            1,
            Dimension::Overworld,
            full(&[(0, Vec::new())]),
        );
        let owners = [(WEST, 1), (EAST, 1), (RegionId(2), 1)]
            .into_iter()
            .collect();
        (village, owners)
    }

    /// The plan, sections 19.5 and 19.6: the fight's bar as the End's home says it is
    /// published to the End's other regions only, and the home is told when another
    /// region of the End has a player in the fight's reach.
    #[test]
    fn the_fight_s_bar_goes_to_the_end_s_other_regions_and_the_arena_s_players_to_the_home() {
        let (mut village, owners) = the_end_and_the_overworld();
        let bar = FightSeen {
            bar: uuid::Uuid::from_u128(7),
            shown: true,
            progress: 0.5,
        };
        let report = |fight, in_arena| VillageReport {
            fight,
            in_arena,
            ..VillageReport::default()
        };
        village.report(WEST, 1, Dimension::End, report(Some(bar), None));
        village.report(EAST, 1, Dimension::End, report(None, Some(true)));
        let to = |village: &mut Village, region| {
            village
                .publication(region, 1, nothing_copied, &owners)
                .expect("a first publication")
        };
        let east = to(&mut village, EAST);
        assert_eq!((east.fight, east.arena_wanted), (Some(bar), false));
        let home = to(&mut village, WEST);
        assert_eq!((home.fight, home.arena_wanted), (None, true));
        let overworld = to(&mut village, RegionId(2));
        assert_eq!((overworld.fight, overworld.arena_wanted), (None, false));
        // Nothing new for the home until the other region's player leaves the reach.
        assert_eq!(village.publication(WEST, 1, nothing_copied, &owners), None);
        village.report(EAST, 1, Dimension::End, report(None, Some(false)));
        let home = village.publication(WEST, 1, nothing_copied, &owners);
        assert_eq!(home.map(|home| home.arena_wanted), Some(false));
    }

    /// The plan, section 19.5 item 8 and 19.6: a global level event is published to
    /// every region, of every dimension, numbered, once to each owner, from that owner's
    /// first publication on.
    #[test]
    fn a_global_level_event_is_published_once_to_every_region_with_its_number() {
        let (mut village, owners) = the_end_and_the_overworld();
        for region in [WEST, EAST, RegionId(2)] {
            village.publication(region, 1, nothing_copied, &owners);
        }
        let opened = GlobalLevelEvent {
            event: 1038,
            dimension: Dimension::Overworld,
            position: BlockPos::new(1, 30, -700),
            data: 0,
        };
        village.report(
            RegionId(2),
            1,
            Dimension::Overworld,
            VillageReport {
                global_events: vec![opened],
                ..VillageReport::default()
            },
        );
        for region in [WEST, EAST, RegionId(2)] {
            let published = village
                .publication(region, 1, nothing_copied, &owners)
                .expect("the event is news");
            assert_eq!(published.global_events, [(1, opened)], "{region}");
            assert_eq!(
                village.publication(region, 1, nothing_copied, &owners),
                None
            );
        }
        // A new owner of a region is told what is made from its first publication on.
        let mut owners = owners;
        owners.insert(EAST, 2);
        village.report(EAST, 2, Dimension::End, full(&[(40, Vec::new())]));
        let anew = village
            .publication(EAST, 2, nothing_copied, &owners)
            .unwrap();
        assert!(anew.global_events.is_empty());
    }

    #[test]
    fn a_region_is_published_whole_first_then_only_what_changed() {
        let mut village = two_regions();
        let first = village
            .publication(WEST, 1, nothing_copied, &Default::default())
            .unwrap();
        assert!(first.whole);
        assert_eq!(first.pois, [(ChunkPos::new(1, 0), vec![bed(20, true)])]);
        assert_eq!(
            village.publication(WEST, 1, nothing_copied, &Default::default()),
            None
        );

        village.report(
            EAST,
            1,
            Dimension::Overworld,
            VillageReport {
                fight: None,
                in_arena: None,
                global_events: Vec::new(),
                full: false,
                pois: vec![(ChunkPos::new(1, 0), vec![bed(20, false)])],
                let_go: Vec::new(),
                raids: None,
            },
        );
        let second = village
            .publication(WEST, 1, nothing_copied, &Default::default())
            .unwrap();
        assert!(!second.whole);
        assert_eq!(second.pois, [(ChunkPos::new(1, 0), vec![bed(20, false)])]);

        village.report(
            EAST,
            1,
            Dimension::Overworld,
            VillageReport {
                fight: None,
                in_arena: None,
                global_events: Vec::new(),
                full: false,
                pois: vec![(ChunkPos::new(1, 0), Vec::new())],
                let_go: Vec::new(),
                raids: None,
            },
        );
        let third = village
            .publication(WEST, 1, nothing_copied, &Default::default())
            .unwrap();
        assert_eq!(third.dropped, [ChunkPos::new(1, 0)]);
        // A region is never published its own land.
        assert_eq!(
            village
                .publication(EAST, 1, nothing_copied, &Default::default())
                .unwrap()
                .pois,
            []
        );
    }

    #[test]
    fn a_copied_chunk_and_one_beyond_five_chunks_are_not_published() {
        let mut village = two_regions();
        let copied = |chunk: LevelChunk| chunk.position == ChunkPos::new(1, 0);
        assert_eq!(
            village
                .publication(WEST, 1, copied, &Default::default())
                .unwrap()
                .pois,
            []
        );
        let mut far = Village::default();
        far.report(WEST, 1, Dimension::Overworld, full(&[(-5, Vec::new())]));
        far.report(
            EAST,
            1,
            Dimension::Overworld,
            full(&[(1, vec![bed(20, true)])]),
        );
        assert_eq!(
            far.publication(WEST, 1, nothing_copied, &Default::default())
                .unwrap()
                .pois,
            []
        );
    }

    #[test]
    fn a_new_owner_s_full_report_makes_the_next_publication_whole_and_an_old_one_is_let_go() {
        let mut village = two_regions();
        village
            .publication(WEST, 1, nothing_copied, &Default::default())
            .unwrap();
        // The region opened again by another owner: until its full report, nothing.
        assert_eq!(
            village.publication(WEST, 2, nothing_copied, &Default::default()),
            None
        );
        village.report(WEST, 2, Dimension::Overworld, full(&[(-1, Vec::new())]));
        assert!(
            village
                .publication(WEST, 2, nothing_copied, &Default::default())
                .unwrap()
                .whole
        );
        // A partial report of the owner before is let go.
        village.report(
            WEST,
            1,
            Dimension::Overworld,
            VillageReport {
                fight: None,
                in_arena: None,
                global_events: Vec::new(),
                full: false,
                pois: vec![(ChunkPos::new(-4, 0), vec![bed(-60, true)])],
                let_go: Vec::new(),
                raids: None,
            },
        );
        assert_eq!(
            village
                .publication(EAST, 1, nothing_copied, &Default::default())
                .unwrap()
                .pois,
            []
        );
    }

    /// A merge or a split restores a region within its owner's session: its first tick
    /// reports its land whole, and its margin is published whole again, since the region
    /// began anew without it.
    #[test]
    fn a_full_report_of_the_same_owner_makes_the_next_publication_whole() {
        let mut village = two_regions();
        village
            .publication(WEST, 1, nothing_copied, &Default::default())
            .unwrap();
        assert_eq!(
            village.publication(WEST, 1, nothing_copied, &Default::default()),
            None
        );
        village.report(
            WEST,
            1,
            Dimension::Overworld,
            full(&[(-3, Vec::new()), (-2, Vec::new()), (-1, Vec::new())]),
        );
        let again = village
            .publication(WEST, 1, nothing_copied, &Default::default())
            .unwrap();
        assert!(again.whole);
        assert_eq!(again.pois, [(ChunkPos::new(1, 0), vec![bed(20, true)])]);
    }

    #[test]
    fn a_chunk_let_go_keeps_its_records_until_another_region_reports_it() {
        let mut village = two_regions();
        village.report(
            EAST,
            1,
            Dimension::Overworld,
            VillageReport {
                fight: None,
                in_arena: None,
                global_events: Vec::new(),
                full: false,
                pois: Vec::new(),
                let_go: vec![ChunkPos::new(1, 0)],
                raids: None,
            },
        );
        let published = village
            .publication(WEST, 1, nothing_copied, &Default::default())
            .unwrap();
        assert_eq!(published.pois, [(ChunkPos::new(1, 0), vec![bed(20, true)])]);
        // West takes it: it is its land now, and not published to it.
        village.report(
            WEST,
            1,
            Dimension::Overworld,
            VillageReport {
                fight: None,
                in_arena: None,
                global_events: Vec::new(),
                full: false,
                pois: vec![(ChunkPos::new(1, 0), vec![bed(20, true)])],
                let_go: Vec::new(),
                raids: None,
            },
        );
        let after = village
            .publication(WEST, 1, nothing_copied, &Default::default())
            .unwrap();
        assert_eq!(after.dropped, [ChunkPos::new(1, 0)]);
    }

    fn raid(id: u64) -> RaidSeen {
        RaidSeen {
            id: clustine_world::EntityId(id),
            chunk: ChunkPos::new(0, 0),
            centre: BlockPos::new(0, 64, 0),
            active: true,
            status: clustine_world::raid::RaidStatus::Ongoing,
            groups_spawned: 0,
            between_waves: false,
            omen: 1,
            bar: uuid::Uuid::from_u128(u128::from(id)),
            shown: true,
            progress: 0.0,
            name: clustine_world::raid::RaidBarName::Raid,
        }
    }

    fn raids_of(region: RegionId, session: u64, raids: Vec<RaidSeen>) -> VillageReport {
        let _ = region;
        let _ = session;
        VillageReport {
            fight: None,
            in_arena: None,
            global_events: Vec::new(),
            full: false,
            pois: Vec::new(),
            let_go: Vec::new(),
            raids: Some(raids),
        }
    }

    /// A region is published the raids of every other region its owner of now
    /// reported, of one id the later word, and not its own
    /// (`docs/groundwork/n10-plan.md`, section 9.2).
    #[test]
    fn a_region_is_published_the_others_raids_of_their_owners_now_the_later_word_holding() {
        let mut village = two_regions();
        let owners: BTreeMap<RegionId, u64> = [(WEST, 1), (EAST, 1)].into_iter().collect();
        village.report(
            EAST,
            1,
            Dimension::Overworld,
            raids_of(EAST, 1, vec![raid(7)]),
        );
        let first = village
            .publication(WEST, 1, nothing_copied, &owners)
            .unwrap();
        assert_eq!(first.raids, [raid(7)]);
        let own = village
            .publication(EAST, 1, nothing_copied, &owners)
            .unwrap();
        assert!(own.raids.is_empty(), "never its own");
        // West takes the raid over (a split's part): its later word holds.
        let mut moved = raid(7);
        moved.chunk = ChunkPos::new(-1, 0);
        village.report(
            WEST,
            1,
            Dimension::Overworld,
            raids_of(WEST, 1, vec![moved.clone()]),
        );
        let by_east = village
            .publication(EAST, 1, nothing_copied, &owners)
            .unwrap();
        assert_eq!(by_east.raids, [moved]);
        // A region no longer owned by the owner that reported says nothing.
        let taken_over: BTreeMap<RegionId, u64> = [(WEST, 2), (EAST, 1)].into_iter().collect();
        let after = village
            .publication(EAST, 1, nothing_copied, &taken_over)
            .unwrap();
        assert!(after.raids.is_empty());
    }

    /// The game keeps points of interest and raids by level: a region of the Nether at
    /// the same positions as the overworld's is published neither their beds nor their
    /// raids, and the overworld's regions nothing of it.
    #[test]
    fn a_region_is_published_the_records_and_raids_of_its_own_dimension_only() {
        let mut village = two_regions();
        let nether = RegionId(2);
        village.report(
            nether,
            1,
            Dimension::Nether,
            full(&[(1, vec![bed(30, true)])]),
        );
        village.report(
            nether,
            1,
            Dimension::Nether,
            raids_of(nether, 1, vec![raid(9)]),
        );
        village.report(
            EAST,
            1,
            Dimension::Overworld,
            raids_of(EAST, 1, vec![raid(7)]),
        );
        let owners: BTreeMap<RegionId, u64> =
            [(WEST, 1), (EAST, 1), (nether, 1)].into_iter().collect();
        let west = village
            .publication(WEST, 1, nothing_copied, &owners)
            .unwrap();
        assert_eq!(west.pois, [(ChunkPos::new(1, 0), vec![bed(20, true)])]);
        assert_eq!(west.raids, [raid(7)]);
        // The Nether's region holds its chunk 1, and the overworld's chunks around it
        // are none of its land's neighbours.
        let to_nether = village
            .publication(nether, 1, nothing_copied, &owners)
            .unwrap();
        assert_eq!(to_nether.pois, []);
        assert_eq!(to_nether.raids, []);
        // A second region of the Nether beside it is published its bed.
        let beside = RegionId(3);
        village.report(beside, 1, Dimension::Nether, full(&[(2, Vec::new())]));
        let owners: BTreeMap<RegionId, u64> = [(WEST, 1), (EAST, 1), (nether, 1), (beside, 1)]
            .into_iter()
            .collect();
        let to_beside = village
            .publication(beside, 1, nothing_copied, &owners)
            .unwrap();
        assert_eq!(to_beside.pois, [(ChunkPos::new(1, 0), vec![bed(30, true)])]);
        assert_eq!(to_beside.raids, [raid(9)]);
    }
}
