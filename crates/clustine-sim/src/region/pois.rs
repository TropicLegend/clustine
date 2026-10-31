//! Points of interest and the village (`PoiManager`, `PoiSection`, `PoiRecord`,
//! `ServerLevel.isVillage`; `docs/groundwork/n10-plan.md`, section 3).
//!
//! A record is wherever a block of a type is ([`Chunk::pois`]); a chunk keeps only its
//! claims, the records with fewer free tickets than their type has. The region reads
//! records in its land and its copies (and, from N10b's second step, a margin the store
//! publishes); it takes a ticket only in its land, so that two regions never take one
//! ticket twice (decision 2), and gives one back where it does not hold the chunk by an
//! effect its holder applies as the game's guarded release.
//!
//! **The order of a query**: the game streams a section's records out of hash maps,
//! keyed by the type's identity hash, so its order differs between runs (D10, part A).
//! Here it is fixed (decision 15): chunks x fastest then z, as `ChunkPos.rangeClosed`
//! walks them; in a chunk, sections bottom to top; in a section, by the type's id and
//! then by position (y, z, x). A query is so a function of the state, and a rule that
//! takes the first of a query is judged by distribution.

#![allow(
    dead_code,
    reason = "the villagers' and the village goals' parts of N10 call these (V1, V2, G1)"
)]

use clustine_world::poi::PoiType;
use clustine_world::{BlockPos, Chunk, ChunkPos, Effect, Effected, InChunk};

use super::Random;
use super::Region;
use crate::api::{PoiSeen, PublishedVillage, TickOutput, VillageReport};

/// Which records a query wants by their tickets (`PoiManager.Occupancy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Occupancy {
    /// A free ticket (`HAS_SPACE`).
    HasSpace,
    /// Not every ticket free (`IS_OCCUPIED`).
    IsOccupied,
    Any,
}

/// A record as a query sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoiRecord {
    pub(crate) pos: BlockPos,
    pub(crate) kind: PoiType,
    pub(crate) free: u8,
    /// In the region's land, where a ticket may be taken.
    pub(crate) own: bool,
}

impl PoiRecord {
    fn has_space(&self) -> bool {
        self.free > 0
    }

    fn is_occupied(&self) -> bool {
        self.free != self.kind.max_tickets()
    }

    fn fits(&self, occupancy: Occupancy) -> bool {
        match occupancy {
            // A ticket is taken only in the land: a record anywhere else has no space
            // for this region (decision 2).
            Occupancy::HasSpace => self.own && self.has_space(),
            Occupancy::IsOccupied => self.is_occupied(),
            Occupancy::Any => true,
        }
    }

    /// `BlockPos.distSqr` from the block `from`.
    pub(crate) fn dist_sqr(&self, from: BlockPos) -> f64 {
        dist_sqr(self.pos, from)
    }
}

pub(crate) fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let dx = f64::from(a.x) - f64::from(b.x);
    let dy = f64::from(a.y) - f64::from(b.y);
    let dz = f64::from(a.z) - f64::from(b.z);
    dx * dx + dy * dy + dz * dz
}

/// The section a block is in, as `SectionPos.of`.
pub(crate) fn section_of(pos: BlockPos) -> [i32; 3] {
    [pos.x >> 4, pos.y >> 4, pos.z >> 4]
}

/// How far, in sections, a section is from the nearest that holds an occupied record of
/// `#village` when that is at most this (`SectionTracker(7, 16, 256)`, D10 part A,
/// correction 1); farther is [`NO_VILLAGE`].
const VILLAGE_REACH: i32 = 5;
/// The level of a section no village reaches.
pub(crate) const NO_VILLAGE: i32 = 7;

/// Whether the village is read from the claims of points of interest, which villagers
/// make; off, from the stand-in of the kept villages' pieces that served before they did
/// (`docs/groundwork/n10-plan.md`, section 3.4).
const VILLAGES_CLAIMED: bool = true;

/// A chunk's records in a query's order (sections bottom to top, then type, then
/// position), each with its free tickets.
fn chunk_records(position: ChunkPos, chunk: &Chunk, own: bool) -> Vec<PoiRecord> {
    let mut records: Vec<(i32, PoiType, InChunk)> = chunk
        .pois()
        .iter()
        .map(|&(at, kind)| (i32::from(at.y) >> 4, kind, at))
        .collect();
    records.sort_unstable();
    records
        .into_iter()
        .map(|(_, kind, at)| PoiRecord {
            pos: BlockPos::new(
                position.x * 16 + i32::from(at.x),
                i32::from(at.y),
                position.z * 16 + i32::from(at.z),
            ),
            kind,
            free: chunk.free_tickets(at).unwrap_or(0),
            own,
        })
        .collect()
}

/// A chunk's records as the margin publishes them, in a query's order: a ticket of one is
/// never taken here, and one occupied counts as having none free.
fn margin_records(seen: &[PoiSeen]) -> Vec<PoiRecord> {
    let mut records: Vec<PoiRecord> = seen
        .iter()
        .map(|seen| PoiRecord {
            pos: seen.at,
            kind: seen.kind,
            free: if seen.occupied {
                0
            } else {
                seen.kind.max_tickets()
            },
            own: false,
        })
        .collect();
    records.sort_unstable_by_key(|record| {
        (
            record.pos.y >> 4,
            record.kind,
            record.pos.y,
            record.pos.z,
            record.pos.x,
        )
    });
    records
}

/// A chunk's records as others read them: each record, and whether a ticket is taken.
fn seen_of(position: ChunkPos, chunk: &Chunk) -> Vec<PoiSeen> {
    chunk_records(position, chunk, false)
        .into_iter()
        .map(|record| PoiSeen {
            at: record.pos,
            kind: record.kind,
            occupied: record.is_occupied(),
        })
        .collect()
}

impl Region {
    /// The chunk at `position` as points of interest are read: the land's, else a
    /// copy's, and whether it is the land's.
    fn poi_chunk(&self, position: ChunkPos) -> Option<(&Chunk, bool)> {
        if let Some(chunk) = self.chunks.get(&position) {
            return Some((chunk, true));
        }
        self.copies.get(&position).map(|chunk| (chunk, false))
    }

    /// The records of the chunk at `position` in a query's order, from the land, a copy
    /// or the margin, if the region knows them.
    fn records_of(&self, position: ChunkPos) -> Option<Vec<PoiRecord>> {
        if let Some((chunk, own)) = self.poi_chunk(position) {
            return Some(chunk_records(position, chunk, own));
        }
        self.margin.get(&position).map(|seen| margin_records(seen))
    }

    /// Takes what the store published of the margin (section 3.5): whole, it replaces
    /// what the region had; else its changes are laid over it.
    pub(super) fn take_margin(&mut self, published: &PublishedVillage) {
        if published.whole {
            self.margin.clear();
        }
        for position in &published.dropped {
            self.margin.remove(position);
        }
        for (position, seen) in &published.pois {
            self.margin.insert(*position, seen.clone());
        }
        // The world's raids come whole with it (`raids.rs`), and the End's news
        // (`end_entities.rs`).
        self.take_raid_table(&published.raids);
        self.take_published_end(published);
    }

    /// What the tick says of the land's points of interest (section 3.5): each chunk whose
    /// records or occupancy changed, or that came into the land, whole; the chunks let
    /// go; or, in a region's first tick after it is made or restored, every chunk.
    pub(super) fn report_village(&mut self, full: bool, output: &mut TickOutput) {
        let mut report = VillageReport {
            full,
            ..VillageReport::default()
        };
        if full {
            self.reported_pois.clear();
        }
        let chunks = &self.chunks;
        let reported = &mut self.reported_pois;
        let gone: Vec<ChunkPos> = reported
            .keys()
            .filter(|position| !chunks.contains_key(position))
            .copied()
            .collect();
        for position in gone {
            reported.remove(&position);
            report.let_go.push(position);
        }
        for (position, chunk) in chunks {
            let seen = seen_of(*position, chunk);
            if reported.get(position) != Some(&seen) {
                report.pois.push((*position, seen.clone()));
                reported.insert(*position, seen);
            }
        }
        report.raids = self.report_raids(full);
        if full || !report.pois.is_empty() || !report.let_go.is_empty() || report.raids.is_some() {
            output.village = Some(report);
        }
    }

    /// Every record in the chunks `rangeClosed(chunkOf(centre), floorDiv(r, 16) + 1)`
    /// whose block is within `r` of `centre` on x and on z, at any height
    /// (`PoiManager.getInSquare`), in the query's order.
    pub(crate) fn pois_in_square(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: impl Fn(PoiType) -> bool,
        occupancy: Occupancy,
    ) -> Vec<PoiRecord> {
        let reach = r.div_euclid(16) + 1;
        let middle = centre.chunk();
        let mut found = Vec::new();
        for z in middle.z - reach..=middle.z + reach {
            for x in middle.x - reach..=middle.x + reach {
                let position = ChunkPos::new(x, z);
                let Some(records) = self.records_of(position) else {
                    continue;
                };
                found.extend(records.into_iter().filter(|record| {
                    (record.pos.x - centre.x).abs() <= r
                        && (record.pos.z - centre.z).abs() <= r
                        && wanted(record.kind)
                        && record.fits(occupancy)
                }));
            }
        }
        found
    }

    /// The records within `r` of `centre` (`PoiManager.getInRange`: `distSqr <= r²`).
    pub(crate) fn pois_in_range(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: impl Fn(PoiType) -> bool,
        occupancy: Occupancy,
    ) -> Vec<PoiRecord> {
        let most = f64::from(r) * f64::from(r);
        let mut found = self.pois_in_square(centre, r, wanted, occupancy);
        found.retain(|record| record.dist_sqr(centre) <= most);
        found
    }

    /// `findAllClosestFirstWithType`: the records within `r`, nearest first, those at one
    /// distance in the query's order (a stable sort).
    pub(crate) fn pois_closest_first(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: impl Fn(PoiType) -> bool,
        occupancy: Occupancy,
    ) -> Vec<PoiRecord> {
        let mut found = self.pois_in_range(centre, r, wanted, occupancy);
        found.sort_by(|a, b| a.dist_sqr(centre).total_cmp(&b.dist_sqr(centre)));
        found
    }

    /// `findClosest`: the nearest record within `r`, the first of those at one distance.
    pub(crate) fn poi_closest(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: impl Fn(PoiType) -> bool,
        occupancy: Occupancy,
    ) -> Option<PoiRecord> {
        self.pois_closest_first(centre, r, wanted, occupancy)
            .into_iter()
            .next()
    }

    /// `PoiManager.take`: the first record within `r` of `centre` in the query's order,
    /// of a wanted type, with a free ticket and passing `test`, whose ticket is taken.
    /// Only the land's records have space for this region (decision 2).
    pub(crate) fn take_poi(
        &mut self,
        centre: BlockPos,
        r: i32,
        wanted: impl Fn(PoiType) -> bool,
        test: impl Fn(&PoiRecord) -> bool,
        output: &mut TickOutput,
    ) -> Option<PoiRecord> {
        let record = self
            .pois_in_range(centre, r, wanted, Occupancy::HasSpace)
            .into_iter()
            .find(|record| test(record))?;
        // `acquireTicket`'s answer is ignored by the game's `take`.
        let _ = self.acquire_poi(record.pos, output);
        Some(record)
    }

    /// `PoiManager.getRandom`: the records within `r` shuffled as `Util.toShuffledList`
    /// shuffles (one `nextInt(i)` for `i` from the count down to 2), the first passing
    /// `test`.
    pub(crate) fn random_poi(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: impl Fn(PoiType) -> bool,
        occupancy: Occupancy,
        test: impl Fn(&PoiRecord) -> bool,
        random: &mut Random,
    ) -> Option<PoiRecord> {
        let mut found = self.pois_in_range(centre, r, wanted, occupancy);
        for i in (2..=found.len()).rev() {
            let j = random.next_int(i as u32) as usize;
            found.swap(i - 1, j);
        }
        found.into_iter().find(|record| test(record))
    }

    /// The type of the record at `pos`, where the region can read it: its land, its
    /// copies and the margin.
    pub(crate) fn poi_type_at(&self, pos: BlockPos) -> Option<PoiType> {
        if let Some((chunk, _)) = self.poi_chunk(pos.chunk()) {
            return chunk.poi_type_at(in_chunk(pos));
        }
        self.margin
            .get(&pos.chunk())?
            .iter()
            .find(|seen| seen.at == pos)
            .map(|seen| seen.kind)
    }

    /// `PoiManager.exists(pos, test)`: a record at `pos` whose type passes `wanted`.
    pub(crate) fn poi_exists(&self, pos: BlockPos, wanted: impl Fn(PoiType) -> bool) -> bool {
        self.poi_type_at(pos).is_some_and(wanted)
    }

    /// Takes a ticket of the record at `pos` in the land (`PoiRecord.acquireTicket`).
    pub(crate) fn acquire_poi(&mut self, pos: BlockPos, output: &mut TickOutput) -> bool {
        let Some(chunk) = self.chunks.get_mut(&pos.chunk()) else {
            return false;
        };
        let taken = chunk.acquire_ticket(in_chunk(pos));
        if taken {
            output.claims_changed.push(pos.chunk());
        }
        taken
    }

    /// `PoiManager.release`: gives a ticket of the record at `pos` of type `kind` back,
    /// guarded as the game's callers guard it (the type still there). In the land it is
    /// given back now; elsewhere the chunk's holder is sent it (decision 2).
    pub(crate) fn release_poi(&mut self, pos: BlockPos, kind: PoiType, output: &mut TickOutput) {
        if self.chunks.contains_key(&pos.chunk()) {
            self.release_held(pos, kind, output);
        } else if self.sends_to(pos.chunk()) {
            self.made.push(Effect {
                at: pos,
                what: Effected::Release { kind },
            });
        }
    }

    /// A release of a ticket of the record at `pos` in the land, made here or sent by
    /// another region: only where a record of `kind` is there and a ticket is taken.
    pub(crate) fn release_held(&mut self, pos: BlockPos, kind: PoiType, output: &mut TickOutput) {
        let Some(chunk) = self.chunks.get_mut(&pos.chunk()) else {
            return;
        };
        let at = in_chunk(pos);
        if chunk.poi_type_at(at) == Some(kind) && chunk.release_ticket(at) {
            output.claims_changed.push(pos.chunk());
        }
    }

    /// `PoiManager.sectionsToVillage`: how many sections away, in three dimensions, the
    /// nearest section with an occupied record of `#village` is, 0 to 5, or
    /// [`NO_VILLAGE`]. Read from the land, the copies and the margin.
    pub(crate) fn sections_to_village(&self, section: [i32; 3]) -> i32 {
        let mut nearest = NO_VILLAGE;
        for z in section[2] - VILLAGE_REACH..=section[2] + VILLAGE_REACH {
            for x in section[0] - VILLAGE_REACH..=section[0] + VILLAGE_REACH {
                let Some(records) = self.records_of(ChunkPos::new(x, z)) else {
                    continue;
                };
                for record in records {
                    if !record.kind.is_village() || !record.is_occupied() {
                        continue;
                    }
                    let distance = (x - section[0])
                        .abs()
                        .max((z - section[2]).abs())
                        .max(((record.pos.y >> 4) - section[1]).abs());
                    if distance <= VILLAGE_REACH {
                        nearest = nearest.min(distance);
                    }
                }
            }
        }
        nearest
    }

    /// How far the village is, as every reader of it asks: by the stand-in of the kept
    /// villages' pieces while [`VILLAGES_CLAIMED`] is off, by the claims once villagers
    /// claim (section 3.4): a generated village has no claims until its villagers make
    /// them, and until then the stand-in keeps patrols out of it.
    pub(crate) fn village_level(&self, section: [i32; 3]) -> i32 {
        if VILLAGES_CLAIMED {
            return self.sections_to_village(section);
        }
        let centre = BlockPos::new(
            section[0] * 16 + 8,
            section[1] * 16 + 8,
            section[2] * 16 + 8,
        );
        if self.village_near(centre) {
            0
        } else {
            NO_VILLAGE
        }
    }

    /// `ServerLevel.isCloseToVillage(pos, r)` as the region's own readers ask it: through
    /// the stand-in while [`VILLAGES_CLAIMED`] is off (a kept village's piece near `pos`,
    /// `structures.rs`), through the claims after.
    pub(crate) fn close_to_village(&self, pos: BlockPos, r: i32) -> bool {
        if VILLAGES_CLAIMED {
            self.is_close_to_village(pos, r)
        } else {
            r <= 6 && self.village_near(pos)
        }
    }

    /// `ServerLevel.isCloseToVillage(pos, r)`.
    pub(crate) fn is_close_to_village(&self, pos: BlockPos, r: i32) -> bool {
        r <= 6 && self.sections_to_village(section_of(pos)) <= r
    }

    /// `ServerLevel.isVillage(pos)`.
    pub(crate) fn is_village(&self, pos: BlockPos) -> bool {
        self.is_close_to_village(pos, 1)
    }
}

fn in_chunk(pos: BlockPos) -> InChunk {
    let (x, z) = pos.in_chunk();
    InChunk::new(x, pos.y, z)
}
