//! The regions' counts of their mobs by spawning category, and what each region is
//! published of the others' (`docs/groundwork/n9-plan.md`, section 9.2). In memory
//! only, never in the log: a store started again publishes none until every owned region
//! has reported its chunks whole, which its first tick after it is opened does.
//!
//! Each region's set is kept with the owner that reported it: a report of another owner
//! (the region taken over) begins the set anew, and a sum counts a region's set only while
//! that owner still owns it, so a take-over, a merge's absorbed region and a split's
//! parent drop out of the sums without a path of their own. A partial report changes the
//! reporter's entries; a chunk let go comes as nothing.

use std::collections::BTreeMap;

use clustine_region::RegionId;
use clustine_world::ChunkPos;

use clustine_rpc::{CountsReport, PublishedCounts, SPAWN_CATEGORIES, SpawnCounts, WorldPlayer};

/// How far from a player a chunk's centre counts towards their local counts: 128 blocks
/// horizontally (`(cx·16 + 8 − px)² + (cz·16 + 8 − pz)² < 16384`, D9 5.1).
const LOCAL_RANGE_SQR: f64 = 16_384.0;

/// The spectator's game mode, whose players count nothing near them.
const SPECTATOR: u8 = 3;

/// One region's counts, of the owner session that reported them.
#[derive(Debug, Default)]
struct RegionCounts {
    session: u64,
    by_chunk: BTreeMap<ChunkPos, SpawnCounts>,
}

/// Every region's counts.
#[derive(Debug, Default)]
pub(crate) struct Counts {
    regions: BTreeMap<RegionId, RegionCounts>,
}

impl Counts {
    /// `report` of the region `region`'s owner of the session `session`. A full report
    /// replaces the region's set; a partial one of the owner the set is of changes its
    /// chunks, and one of another owner is let go (its full report comes first).
    pub(crate) fn report(&mut self, region: RegionId, session: u64, report: CountsReport) {
        if report.full {
            let by_chunk = report
                .chunks
                .into_iter()
                .filter(|(_, counts)| counts.iter().any(|count| *count > 0))
                .collect();
            self.regions
                .insert(region, RegionCounts { session, by_chunk });
            return;
        }
        let Some(kept) = self
            .regions
            .get_mut(&region)
            .filter(|kept| kept.session == session)
        else {
            return;
        };
        for (chunk, counts) in report.chunks {
            if counts.iter().all(|count| *count == 0) {
                kept.by_chunk.remove(&chunk);
            } else {
                kept.by_chunk.insert(chunk, counts);
            }
        }
    }

    /// What `region` is published of the other regions, the owners of the regions being
    /// `owners` (each region and its owner's session): none unless every owned region
    /// has reported as its owner now.
    pub(crate) fn published(
        &self,
        region: RegionId,
        owners: &BTreeMap<RegionId, u64>,
        players: &[WorldPlayer],
    ) -> Option<PublishedCounts> {
        let reported = owners.iter().all(|(owned, session)| {
            self.regions
                .get(owned)
                .is_some_and(|kept| kept.session == *session)
        });
        if !reported {
            return None;
        }
        let others = || {
            owners
                .keys()
                .filter(move |other| **other != region)
                .filter_map(|other| self.regions.get(other))
                .flat_map(|kept| kept.by_chunk.iter())
        };
        let mut totals = [0; SPAWN_CATEGORIES];
        for (_, counts) in others() {
            add(&mut totals, counts);
        }
        let players = players
            .iter()
            .filter(|player| player.game_mode != SPECTATOR)
            .map(|player| {
                let mut near = [0; SPAWN_CATEGORIES];
                for (chunk, counts) in others() {
                    let dx = f64::from(chunk.x * 16 + 8) - player.position[0];
                    let dz = f64::from(chunk.z * 16 + 8) - player.position[2];
                    if dx * dx + dz * dz < LOCAL_RANGE_SQR {
                        add(&mut near, counts);
                    }
                }
                (player.player, near)
            })
            .collect();
        Some(PublishedCounts { totals, players })
    }
}

fn add(into: &mut SpawnCounts, counts: &SpawnCounts) {
    for (total, count) in into.iter_mut().zip(counts) {
        *total += count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_world::{EntityId, PlayerId};

    fn region(number: u32) -> RegionId {
        RegionId(number)
    }

    fn report(full: bool, chunks: &[(i32, i32, u32)]) -> CountsReport {
        CountsReport {
            full,
            chunks: chunks
                .iter()
                .map(|(x, z, monsters)| (ChunkPos::new(*x, *z), [*monsters, 0, 0, 0, 0, 0, 0]))
                .collect(),
        }
    }

    fn player(number: u128, x: f64, z: f64, game_mode: u8) -> WorldPlayer {
        WorldPlayer {
            player: PlayerId(uuid::Uuid::from_u128(number)),
            entity: EntityId(u64::try_from(number).unwrap()),
            position: [x, 64.0, z],
            game_mode,
            sleeping: None,
        }
    }

    #[test]
    fn a_region_is_published_the_others_totals_and_its_own_are_left_out() {
        let mut counts = Counts::default();
        counts.report(region(1), 7, report(true, &[(0, 0, 3)]));
        counts.report(region(2), 8, report(true, &[(5, 0, 4), (6, 0, 1)]));
        let owners = BTreeMap::from([(region(1), 7), (region(2), 8)]);
        let to_one = counts.published(region(1), &owners, &[]).unwrap();
        assert_eq!(to_one.totals[0], 5);
        let to_two = counts.published(region(2), &owners, &[]).unwrap();
        assert_eq!(to_two.totals[0], 3);
    }

    #[test]
    fn nothing_is_published_until_every_owned_region_has_reported_whole() {
        let mut counts = Counts::default();
        counts.report(region(1), 7, report(true, &[(0, 0, 3)]));
        let owners = BTreeMap::from([(region(1), 7), (region(2), 8)]);
        assert_eq!(counts.published(region(1), &owners, &[]), None);
        // A partial report before the full one counts for nothing.
        counts.report(region(2), 8, report(false, &[(5, 0, 4)]));
        assert_eq!(counts.published(region(1), &owners, &[]), None);
        counts.report(region(2), 8, report(true, &[(5, 0, 4)]));
        assert!(counts.published(region(1), &owners, &[]).is_some());
    }

    #[test]
    fn a_region_taken_over_counts_only_from_its_new_owner_s_full_report() {
        let mut counts = Counts::default();
        counts.report(region(1), 7, report(true, &[(0, 0, 3)]));
        counts.report(region(2), 8, report(true, &[(5, 0, 4)]));
        // Region 2 is taken over by session 9, which has not reported yet: no sums.
        let owners = BTreeMap::from([(region(1), 7), (region(2), 9)]);
        assert_eq!(counts.published(region(1), &owners, &[]), None);
        // The old owner's late word changes nothing.
        counts.report(region(2), 8, report(false, &[(5, 0, 9)]));
        counts.report(region(2), 9, report(true, &[(5, 0, 2)]));
        assert_eq!(
            counts.published(region(1), &owners, &[]).unwrap().totals[0],
            2
        );
    }

    #[test]
    fn a_partial_report_changes_its_chunks_and_a_chunk_let_go_comes_as_nothing() {
        let mut counts = Counts::default();
        counts.report(region(1), 7, report(true, &[(0, 0, 3), (1, 0, 2)]));
        counts.report(region(2), 8, report(true, &[]));
        let owners = BTreeMap::from([(region(1), 7), (region(2), 8)]);
        counts.report(region(1), 7, report(false, &[(0, 0, 0), (2, 0, 5)]));
        assert_eq!(
            counts.published(region(2), &owners, &[]).unwrap().totals[0],
            7
        );
    }

    #[test]
    fn a_player_is_published_the_others_counts_within_128_blocks_and_a_spectator_nothing() {
        let mut counts = Counts::default();
        counts.report(region(1), 7, report(true, &[(0, 0, 3)]));
        // Chunk (7, 0)'s centre is at x = 120, inside; (8, 0)'s at 136, outside.
        counts.report(region(2), 8, report(true, &[(7, 0, 4), (8, 0, 1)]));
        let owners = BTreeMap::from([(region(1), 7), (region(2), 8)]);
        let players = [player(1, 0.0, 8.0, 1), player(2, 0.0, 8.0, SPECTATOR)];
        let published = counts.published(region(1), &owners, &players).unwrap();
        assert_eq!(published.players.len(), 1);
        assert_eq!(published.players[0].1[0], 4);
    }
}
