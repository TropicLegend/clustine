//! The table of regions: which regions there are and which chunks each holds.
//!
//! The table is the commit thread's alone. On disk it is the file `regions/table` with
//! the records of the log from the file's `from` on applied to it; see
//! `docs/adr/0011-the-world-store-and-regions.md`.
//!
//! **Who holds a chunk**: the region it is granted to; else, for a chunk of the
//! overworld, the region that is pinned to an area which contains it; else nobody. Areas
//! of different regions never overlap.
//!
//! Each region holds chunks of one dimension for its whole life
//! (`docs/groundwork/p-plan.md`, decision 1), which the table keeps with it: a chunk is
//! named here by its dimension and its position ([`LevelChunk`]), and a region's
//! messages, which name positions alone, are of its dimension. Pins are the
//! overworld's (section 3.3 of the same plan): only overworld regions are pinned, and
//! an area holds nothing of another dimension.

use clustine_rpc::WorldClock;
use std::collections::{BTreeMap, VecDeque};

use clustine_format::{TableFile, TableRegion};
use clustine_region::RegionId;
use clustine_rpc::{ChunkBox, RegionInfo, RegionList};
use clustine_world::{BlockPos, ChunkArea, ChunkPos, Dimension, LevelChunk};

use crate::StoreError;

/// How many absorbed regions the store remembers, with what each went into. A hello for
/// one it has forgotten is refused as for a region that never was.
pub(crate) const ABSORBED_KEPT: usize = 4096;

/// How the world is divided when the store is started: the regions that are pinned to
/// an area, and where players enter the world.
///
/// A world whose regions follow their players is [`Division::open`]: one home region
/// that is pinned to nothing. Regions with a boundary at a known place are
/// [`Division::side_by_side`]. Areas that leave a gap are for tests that need both
/// pinned regions and chunks nobody holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Division {
    /// The chunk players enter the world in.
    pub home: ChunkPos,
    /// The areas of the pinned regions, which must not overlap. Region `i` is pinned
    /// to area `i`.
    pub pinned: Vec<ChunkArea>,
}

/// Why chunk x coordinates are no cuts between regions pinned side by side.
///
/// It reads as what was asked for, so that whoever was handed the coordinates can
/// refuse them in words of their own: "`--pin` takes" and then this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("chunk x coordinates in ascending order without repetitions")]
pub struct NotAscending;

/// The regions a world is pinned to, which whoever starts a store chooses. The home
/// chunk is not among what they choose: it is the chunk of the world's spawn, which
/// the store keeps from when the world was made (`docs/groundwork/t8-plan.md`, T8c).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pins {
    /// The areas, which must not overlap. Region `i` is pinned to area `i`.
    pub areas: Vec<ChunkArea>,
}

impl Pins {
    /// No region is pinned: the regions follow their players.
    pub fn none() -> Self {
        Self::default()
    }

    /// One region pinned to the whole world.
    pub fn whole_world() -> Self {
        Self {
            areas: vec![ChunkArea::EVERYWHERE],
        }
    }

    /// Regions pinned side by side, cut at these chunk x coordinates, as
    /// [`Division::side_by_side`] has them.
    pub fn side_by_side(cuts: &[i32]) -> Result<Self, NotAscending> {
        let areas = Division::side_by_side(ChunkPos::new(0, 0), cuts)?.pinned;
        Ok(Self { areas })
    }

    /// The division of a world whose spawn is in the chunk `home`.
    pub(crate) fn with_home(self, home: ChunkPos) -> Division {
        Division {
            home,
            pinned: self.areas,
        }
    }
}

impl Division {
    /// A world that is one home region, pinned to nothing. It holds the home chunk,
    /// and every other chunk is nobody's until a region claims it.
    pub fn open(home: ChunkPos) -> Self {
        Self {
            home,
            pinned: Vec::new(),
        }
    }

    /// Regions pinned side by side, cut at these chunk x coordinates. `Err` unless
    /// they ascend without repetition.
    ///
    /// Region 0 is pinned to every chunk west of the first cut, each region after it
    /// to the chunks from its cut up to the next, and the last to those from the last
    /// cut on; with no cut, one region is pinned to the whole world. These are the
    /// areas a world had that was divided into stripes at the cuts, so such a world is
    /// found as it was.
    pub fn side_by_side(home: ChunkPos, cuts: &[i32]) -> Result<Self, NotAscending> {
        if cuts.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(NotAscending);
        }
        // Each area begins in the west where the one before it ends in the east.
        let ends = || cuts.iter().copied().map(Some);
        let west = [None].into_iter().chain(ends());
        let east = ends().chain([None]);
        let pinned = west
            .zip(east)
            .map(|(min_x, max_x)| ChunkArea { min_x, max_x })
            .collect();
        Ok(Self { home, pinned })
    }

    /// Refuses a division in which two areas have a chunk in common: who holds it
    /// would depend on the order they are looked at in.
    pub(crate) fn check(&self) -> Result<(), StoreError> {
        for (second, area) in self.pinned.iter().enumerate() {
            if let Some(first) = self.pinned[..second]
                .iter()
                .position(|before| overlap(*before, *area))
            {
                return Err(StoreError::Division {
                    first: RegionId(first as u32),
                    second: RegionId(second as u32),
                });
            }
        }
        Ok(())
    }
}

/// Whether the two areas have a chunk in common.
fn overlap(one: ChunkArea, other: ChunkArea) -> bool {
    let west = |area: ChunkArea| area.min_x.map_or(i64::MIN, i64::from);
    let east = |area: ChunkArea| area.max_x.map_or(i64::MAX, i64::from);
    west(one).max(west(other)) < east(one).min(east(other))
}

/// What a region holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Holding {
    /// The dimension of every chunk the region holds.
    dimension: Dimension,
    /// The areas the region is pinned to: it holds every chunk of them that is not
    /// granted to any region. Only an overworld region has any.
    pinned: Vec<ChunkArea>,
    /// The chunks it was granted, each with the tick of the region it holds it from.
    grants: BTreeMap<LevelChunk, u64>,
}

impl Holding {
    /// Whether one of the region's areas contains `chunk`: never a chunk of another
    /// dimension than the overworld, whose areas they are.
    fn pins(&self, chunk: LevelChunk) -> bool {
        chunk.dimension == Dimension::Overworld
            && self.pinned.iter().any(|area| area.contains(chunk.position))
    }
}

/// The regions there are and the chunks each holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Table {
    /// The first segment of the log whose records are not in the table file.
    pub(crate) from: u64,
    /// The id the next region gets.
    pub(crate) next_region: u32,
    /// The chunk players enter the world in: the overworld's home chunk.
    pub(crate) home_chunk: ChunkPos,
    pub(crate) home_region: RegionId,
    /// The homes of the Nether and of the End, made with the world, never absorbed,
    /// each holding its dimension's chunk (0, 0) (`docs/groundwork/p-plan.md`, decision 2
    /// and section 3.3).
    pub(crate) nether_home: RegionId,
    pub(crate) end_home: RegionId,
    /// The areas of the division the table was made from.
    division: Vec<ChunkArea>,
    /// The living regions.
    regions: BTreeMap<RegionId, Holding>,
    /// Which region each granted chunk is granted to.
    granted: BTreeMap<LevelChunk, RegionId>,
    /// The regions that were absorbed, each with what it went into, oldest first.
    absorbed: VecDeque<(RegionId, RegionId)>,
    /// The world's clock: the later of every region's the store has taken
    /// (`docs/groundwork/n7-plan.md`, section 8.4).
    pub(crate) clock: WorldClock,
    /// The wandering trader spawner's chance and the firing that left it, the latest
    /// firing's the store took (`docs/groundwork/n10-plan.md`, section 9.6).
    pub(crate) trader: Option<(u64, i32)>,
}

impl Table {
    /// The table of a world that is divided as `division` says and has nothing else:
    /// region `i` pinned to area `i`, and the home chunk held by the pinned region
    /// whose area it is in or, if there is none, granted to a region made after them.
    /// Every one of them is the overworld's. Then the homes of the Nether and of the End,
    /// in that order, each granted its dimension's chunk (0, 0). The next region id is
    /// at least `next_region`, so that an id the world has used is not used again for a
    /// region made later.
    pub(crate) fn made_from(division: &Division, next_region: u32, from: u64) -> Self {
        let mut regions: BTreeMap<RegionId, Holding> = (0..)
            .map(RegionId)
            .zip(&division.pinned)
            .map(|(region, area)| {
                let holding = Holding {
                    dimension: Dimension::Overworld,
                    pinned: vec![*area],
                    grants: BTreeMap::new(),
                };
                (region, holding)
            })
            .collect();
        let mut granted = BTreeMap::new();
        let pinned_home = regions
            .iter()
            .find(|(_, holding)| holding.pinned[0].contains(division.home))
            .map(|(region, _)| *region);
        let home = LevelChunk::new(Dimension::Overworld, division.home);
        let home_region = pinned_home.unwrap_or_else(|| {
            let region = RegionId(regions.len() as u32);
            let holding = Holding {
                dimension: Dimension::Overworld,
                pinned: Vec::new(),
                // Ticks are numbered from 1: the chunk is the region's from the start.
                grants: BTreeMap::from([(home, 0)]),
            };
            regions.insert(region, holding);
            granted.insert(home, region);
            region
        });
        let mut home_of = |dimension: Dimension| {
            let region = RegionId(regions.len() as u32);
            let chunk = LevelChunk::new(dimension, ChunkPos::new(0, 0));
            let holding = Holding {
                dimension,
                pinned: Vec::new(),
                grants: BTreeMap::from([(chunk, 0)]),
            };
            regions.insert(region, holding);
            granted.insert(chunk, region);
            region
        };
        let (nether_home, end_home) = (home_of(Dimension::Nether), home_of(Dimension::End));
        Self {
            from,
            next_region: next_region.max(regions.len() as u32),
            home_chunk: division.home,
            home_region,
            nether_home,
            end_home,
            division: division.pinned.clone(),
            regions,
            granted,
            absorbed: VecDeque::new(),
            clock: WorldClock::default(),
            trader: None,
        }
    }

    /// The table as the file has it. A file that passes for one and says what cannot
    /// be, such as a chunk granted to two regions, is refused.
    pub(crate) fn read(file: TableFile) -> Result<Self, StoreError> {
        let mut regions = BTreeMap::new();
        let mut granted = BTreeMap::new();
        for TableRegion {
            id,
            dimension,
            pinned,
            grants,
        } in file.regions
        {
            let region = RegionId(id);
            if id >= file.next_region {
                return Err(misfit(format!(
                    "region {id} is not below the next region id"
                )));
            }
            if dimension != Dimension::Overworld && !pinned.is_empty() {
                return Err(misfit(format!(
                    "region {region} of {} is pinned, and pins are the overworld's",
                    dimension.type_name()
                )));
            }
            let grants: BTreeMap<LevelChunk, u64> = grants
                .into_iter()
                .map(|(chunk, tick)| (LevelChunk::new(dimension, chunk), tick))
                .collect();
            for chunk in grants.keys() {
                if let Some(other) = granted.insert(*chunk, region) {
                    return Err(misfit(format!(
                        "chunk {chunk:?} is granted to regions {other} and {region}"
                    )));
                }
            }
            let holding = Holding {
                dimension,
                pinned,
                grants,
            };
            regions.insert(region, holding);
        }
        let table = Self {
            from: file.from,
            next_region: file.next_region,
            home_chunk: file.home_chunk,
            home_region: RegionId(file.home_region),
            nether_home: RegionId(file.nether_home),
            end_home: RegionId(file.end_home),
            division: file.division,
            regions,
            granted,
            absorbed: file
                .absorbed
                .into_iter()
                .map(|(absorbed, into)| (RegionId(absorbed), RegionId(into)))
                .collect(),
            clock: WorldClock {
                game_time: file.clock[0],
                day: file.clock[1],
            },
            trader: file.trader,
        };
        let areas: Vec<(RegionId, ChunkArea)> = table.areas().collect();
        for (index, (region, area)) in areas.iter().enumerate() {
            let other = areas[..index]
                .iter()
                .find(|(other, before)| other != region && overlap(*before, *area));
            if let Some((other, _)) = other {
                return Err(misfit(format!(
                    "regions {other} and {region} are pinned to areas that overlap"
                )));
            }
        }
        for dimension in Dimension::ALL {
            let home = table.home_region_of(dimension);
            if table.holder(table.home_of(dimension)) != Some(home)
                || table.dimension(home) != Some(dimension)
            {
                return Err(misfit(format!(
                    "the home region {home} of {} does not hold its home chunk",
                    dimension.type_name()
                )));
            }
        }
        Ok(table)
    }

    /// The table as it is written to its file, with `from` as the first segment of the
    /// log that is not in it.
    pub(crate) fn file(&self, from: u64) -> TableFile {
        TableFile {
            from,
            next_region: self.next_region,
            home_chunk: self.home_chunk,
            home_region: self.home_region.0,
            nether_home: self.nether_home.0,
            end_home: self.end_home.0,
            division: self.division.clone(),
            regions: self
                .regions
                .iter()
                .map(|(region, holding)| TableRegion {
                    id: region.0,
                    dimension: holding.dimension,
                    pinned: holding.pinned.clone(),
                    grants: holding
                        .grants
                        .iter()
                        .map(|(chunk, tick)| (chunk.position, *tick))
                        .collect(),
                })
                .collect(),
            absorbed: self
                .absorbed
                .iter()
                .map(|(absorbed, into)| (absorbed.0, into.0))
                .collect(),
            clock: [self.clock.game_time, self.clock.day],
            trader: self.trader,
        }
    }

    /// Whether the table was made from `division`: from the same areas and the same
    /// home chunk. What the regions are pinned to now is not looked at, as a merge
    /// changes that and the division stays the one the store is started with.
    pub(crate) fn is_of(&self, division: &Division) -> bool {
        self.division == division.pinned && self.home_chunk == division.home
    }

    /// The trader's chance as the firing due at `firing` left it: kept unless a later
    /// firing's is. Every region runs each firing, and only the one that holds the
    /// player it chose can make a trader and set the chance back: of one firing, the
    /// lowest chance told stands.
    pub(crate) fn take_trader_chance(&mut self, firing: u64, chance: i32) {
        self.trader = Some(later_trader_chance(self.trader, (firing, chance)));
    }

    /// Whether `region` is a living region.
    pub(crate) fn has(&self, region: RegionId) -> bool {
        self.regions.contains_key(&region)
    }

    /// The dimension of the living region `region`, if it is one.
    pub(crate) fn dimension(&self, region: RegionId) -> Option<Dimension> {
        self.regions.get(&region).map(|holding| holding.dimension)
    }

    /// The dimension of `region`, living or absorbed: a region absorbed is of the
    /// dimension of the region it went into, as no merge joins two dimensions
    /// (`docs/groundwork/p-plan.md`, section 3.4). None for a region the table has
    /// never had, or has forgotten.
    pub(crate) fn dimension_ever(&self, region: RegionId) -> Option<Dimension> {
        let mut region = region;
        // Each step goes to a region absorbed later than the one before, so the chain
        // ends; the bound is for a table read from a file that says otherwise.
        for _ in 0..=self.absorbed.len() {
            if let Some(dimension) = self.dimension(region) {
                return Some(dimension);
            }
            region = self.absorbed_into(region)?;
        }
        None
    }

    /// The overworld's home chunk, where players enter the world.
    pub(crate) fn home(&self) -> LevelChunk {
        LevelChunk::new(Dimension::Overworld, self.home_chunk)
    }

    /// The home chunk of `dimension`, which the region that holds it never gives back:
    /// the spawn's in the overworld and chunk (0, 0) in the others, as
    /// `RegionConfig::home` has it (`docs/groundwork/p-plan.md`, section 15.6).
    pub(crate) fn home_of(&self, dimension: Dimension) -> LevelChunk {
        match dimension {
            Dimension::Overworld => self.home(),
            other => LevelChunk::new(other, ChunkPos::new(0, 0)),
        }
    }

    /// The home region of `dimension`.
    pub(crate) fn home_region_of(&self, dimension: Dimension) -> RegionId {
        match dimension {
            Dimension::Overworld => self.home_region,
            Dimension::Nether => self.nether_home,
            Dimension::End => self.end_home,
        }
    }

    /// Whether `region` is a dimension's home, which is never absorbed.
    pub(crate) fn is_home_region(&self, region: RegionId) -> bool {
        Dimension::ALL
            .into_iter()
            .any(|dimension| self.home_region_of(dimension) == region)
    }

    /// Whether `chunk` is its dimension's home chunk.
    pub(crate) fn is_home(&self, chunk: LevelChunk) -> bool {
        chunk == self.home_of(chunk.dimension)
    }

    /// The living regions, in ascending order.
    pub(crate) fn regions(&self) -> impl Iterator<Item = RegionId> + '_ {
        self.regions.keys().copied()
    }

    /// The areas `region` is pinned to; none if it is not pinned or no region.
    pub(crate) fn pinned(&self, region: RegionId) -> &[ChunkArea] {
        self.regions
            .get(&region)
            .map_or(&[], |holding| &holding.pinned)
    }

    /// Every area a region is pinned to, with the region.
    fn areas(&self) -> impl Iterator<Item = (RegionId, ChunkArea)> + '_ {
        self.regions
            .iter()
            .flat_map(|(region, holding)| holding.pinned.iter().map(|area| (*region, *area)))
    }

    /// The region that holds `chunk`, if one does.
    pub(crate) fn holder(&self, chunk: LevelChunk) -> Option<RegionId> {
        self.granted
            .get(&chunk)
            .copied()
            .or_else(|| self.pinned_holder(chunk))
    }

    /// The region whose pinned area `chunk` is in, if there is one, whoever holds it:
    /// none for a chunk of another dimension than the overworld, whose pins they are
    /// (`docs/groundwork/p-plan.md`, section 3.3, R D3).
    pub(crate) fn pinned_holder(&self, chunk: LevelChunk) -> Option<RegionId> {
        if chunk.dimension != Dimension::Overworld {
            return None;
        }
        self.areas()
            .find(|(_, area)| area.contains(chunk.position))
            .map(|(region, _)| region)
    }

    /// What `region` holds by a lend and has lent away: its grants in another region's
    /// pinned area if it is pinned itself, and the grants of others in its own, each
    /// ascending (`docs/adr/0027-lending-at-a-line.md`, section 1).
    pub(crate) fn lends(&self, region: RegionId) -> (Vec<LevelChunk>, Vec<(LevelChunk, RegionId)>) {
        let holding = self.regions.get(&region);
        let pinned = self.pinned(region);
        // Only a pinned region borrows: a split part's grants in its parent's area are
        // its own by a grant and no lend, which it gives back as any other (section 4).
        let lent = match pinned.is_empty() {
            true => Vec::new(),
            false => self
                .grants(region)
                .into_iter()
                .map(|(chunk, _)| chunk)
                .filter(|chunk| {
                    self.pinned_holder(*chunk)
                        .is_some_and(|area| area != region)
                })
                .collect(),
        };
        let lent_away = self
            .granted
            .iter()
            .filter(|(chunk, holder)| {
                **holder != region && holding.is_some_and(|holding| holding.pins(**chunk))
            })
            .map(|(chunk, holder)| (*chunk, *holder))
            .collect();
        (lent, lent_away)
    }

    /// The tick of `region` from which it holds `chunk`, if it holds it: that of its
    /// grant, or 0 for a chunk it holds by being pinned.
    pub(crate) fn held_from(&self, region: RegionId, chunk: LevelChunk) -> Option<u64> {
        let holding = self.regions.get(&region)?;
        match self.granted.get(&chunk) {
            Some(holder) if *holder == region => holding.grants.get(&chunk).copied(),
            Some(_) => None,
            None => holding.pins(chunk).then_some(0),
        }
    }

    /// The tick of the grant `region` has of `chunk`, if it has one. A chunk it holds
    /// by being pinned is not granted to it.
    pub(crate) fn granted_from(&self, region: RegionId, chunk: LevelChunk) -> Option<u64> {
        self.regions.get(&region)?.grants.get(&chunk).copied()
    }

    /// The grants of `region`, in ascending order of the chunks, each with its tick.
    pub(crate) fn grants(&self, region: RegionId) -> Vec<(LevelChunk, u64)> {
        self.regions.get(&region).map_or_else(Vec::new, |holding| {
            let grants = holding.grants.iter();
            grants.map(|(chunk, tick)| (*chunk, *tick)).collect()
        })
    }

    /// Grants `region` each of `chunks` from its tick `tick` on. Nothing is granted if
    /// there is no such region, one of the chunks is granted already, to whomever, or
    /// one is of another dimension than the region's: that is a record of the log that
    /// does not fit the table.
    pub(crate) fn grant(
        &mut self,
        region: RegionId,
        tick: u64,
        chunks: &[LevelChunk],
    ) -> Result<(), StoreError> {
        if let Some((chunk, holder)) = chunks
            .iter()
            .find_map(|chunk| Some((chunk, self.granted.get(chunk)?)))
        {
            return Err(misfit(format!(
                "chunk {chunk:?} is granted to region {region} while region {holder} has it"
            )));
        }
        let Some(holding) = self.regions.get_mut(&region) else {
            return Err(misfit(format!(
                "chunks are granted to region {region}, which is none"
            )));
        };
        if let Some(chunk) = chunks
            .iter()
            .find(|chunk| chunk.dimension != holding.dimension)
        {
            return Err(misfit(format!(
                "chunk {chunk:?} is granted to region {region} of {}",
                holding.dimension.type_name()
            )));
        }
        for chunk in chunks {
            holding.grants.insert(*chunk, tick);
            self.granted.insert(*chunk, region);
        }
        Ok(())
    }

    /// Takes the grant `region` has of `chunk` from it, and returns the tick it had.
    /// The chunk is then nobody's, or the pinned region's whose area it is in. Returns
    /// `None`, and changes nothing, if the region has no grant of the chunk.
    pub(crate) fn release(&mut self, region: RegionId, chunk: LevelChunk) -> Option<u64> {
        let tick = self.regions.get_mut(&region)?.grants.remove(&chunk)?;
        self.granted.remove(&chunk);
        Some(tick)
    }

    /// The region that `region` was absorbed by, if it was and the store remembers.
    pub(crate) fn absorbed_into(&self, region: RegionId) -> Option<RegionId> {
        let pair = self
            .absorbed
            .iter()
            .rev()
            .find(|(absorbed, _)| *absorbed == region);
        pair.map(|(_, into)| *into)
    }

    /// The merge: every chunk `absorbed` was granted is granted to `region` with
    /// `tick`, every area it was pinned to is one `region` is pinned to, and `absorbed`
    /// is no region any more. Returns the chunks that are granted to `region` by it, in
    /// ascending order. Nothing changes if one of the two is no living region, they are
    /// one or they are of two dimensions: that is a record of the log that does not fit
    /// the table.
    pub(crate) fn absorb(
        &mut self,
        region: RegionId,
        absorbed: RegionId,
        tick: u64,
    ) -> Result<Vec<LevelChunk>, StoreError> {
        let two_dimensions = self
            .dimension(region)
            .zip(self.dimension(absorbed))
            .is_some_and(|(survivor, gone)| survivor != gone);
        if region == absorbed
            || !self.has(region)
            || self.is_home_region(absorbed)
            || two_dimensions
        {
            return Err(misfit(format!(
                "region {absorbed} is absorbed by region {region}, which cannot be"
            )));
        }
        let Some(gone) = self.regions.remove(&absorbed) else {
            return Err(misfit(format!(
                "region {absorbed}, which is none, is absorbed by region {region}"
            )));
        };
        let survivor = self.regions.get_mut(&region).expect("looked up above");
        survivor.pinned.extend(gone.pinned);
        let chunks: Vec<LevelChunk> = gone.grants.into_keys().collect();
        for chunk in &chunks {
            survivor.grants.insert(*chunk, tick);
            self.granted.insert(*chunk, region);
        }
        self.absorbed.push_back((absorbed, region));
        while self.absorbed.len() > ABSORBED_KEPT {
            self.absorbed.pop_front();
        }
        Ok(chunks)
    }

    /// The split: `part` is a new region of `region`'s dimension, not pinned, that
    /// holds `chunks` from `tick` on, which `region` no longer holds, whether it was
    /// granted them or held them by being pinned. Nothing changes if `part` is an id
    /// that has been used, `region` is no living region, or it does not hold one of the
    /// chunks: that is a record of the log that does not fit the table.
    pub(crate) fn split(
        &mut self,
        region: RegionId,
        part: RegionId,
        tick: u64,
        chunks: &[LevelChunk],
    ) -> Result<(), StoreError> {
        if part.0 < self.next_region || part.0 == u32::MAX {
            return Err(misfit(format!(
                "region {part} is split off region {region}, and its id has been used"
            )));
        }
        if !self.has(region) {
            return Err(misfit(format!(
                "region {part} is split off region {region}, which is none"
            )));
        }
        if let Some(chunk) = chunks
            .iter()
            .find(|chunk| self.held_from(region, **chunk).is_none())
        {
            return Err(misfit(format!(
                "chunk {chunk:?} goes to region {part}, and region {region} does not hold it"
            )));
        }
        let old = self.regions.get_mut(&region).expect("it holds chunks");
        let mut holding = Holding {
            dimension: old.dimension,
            ..Holding::default()
        };
        for chunk in chunks {
            old.grants.remove(chunk);
            holding.grants.insert(*chunk, tick);
            self.granted.insert(*chunk, part);
        }
        self.regions.insert(part, holding);
        self.next_region = part.0 + 1;
        Ok(())
    }

    /// The list of regions for whoever assigns them, in a world with this spawn.
    /// `epoch` says the highest epoch a region was opened with.
    pub(crate) fn list(&self, spawn: BlockPos, epoch: impl Fn(RegionId) -> u64) -> RegionList {
        RegionList {
            home: self.home_region,
            homes: Dimension::ALL
                .into_iter()
                .map(|dimension| (dimension, self.home_region_of(dimension)))
                .collect(),
            spawn,
            regions: self
                .regions
                .iter()
                .map(|(region, holding)| RegionInfo {
                    region: *region,
                    dimension: holding.dimension,
                    epoch: epoch(*region),
                    bounds: bounds(holding.grants.keys().map(|chunk| chunk.position)),
                    pinned: holding.pinned.clone(),
                })
                .collect(),
            absorbed: self.absorbed.iter().copied().collect(),
            next: RegionId(self.next_region),
        }
    }
}

/// The smallest box around `chunks`, if there are any.
fn bounds(chunks: impl Iterator<Item = ChunkPos>) -> Option<ChunkBox> {
    chunks.fold(None, |bounds: Option<ChunkBox>, chunk| {
        Some(match bounds {
            None => ChunkBox {
                min: chunk,
                max: chunk,
            },
            Some(ChunkBox { min, max }) => ChunkBox {
                min: ChunkPos::new(min.x.min(chunk.x), min.z.min(chunk.z)),
                max: ChunkPos::new(max.x.max(chunk.x), max.z.max(chunk.z)),
            },
        })
    })
}

fn misfit(what: String) -> StoreError {
    StoreError::Table(what)
}

/// Of two of the trader's chances with their firings, the one that stands: the later
/// firing's, and of one firing the lower (`Table::take_trader_chance`).
fn later_trader_chance(kept: Option<(u64, i32)>, told: (u64, i32)) -> (u64, i32) {
    match kept {
        Some(kept) if kept.0 > told.0 || (kept.0 == told.0 && kept.1 <= told.1) => kept,
        _ => told,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(min_x: Option<i32>, max_x: Option<i32>) -> ChunkArea {
        ChunkArea { min_x, max_x }
    }

    /// The overworld's chunk at `x`, `z`.
    fn o(x: i32, z: i32) -> LevelChunk {
        LevelChunk::new(Dimension::Overworld, ChunkPos::new(x, z))
    }

    /// The Nether's chunk at `x`, `z`.
    fn n(x: i32, z: i32) -> LevelChunk {
        LevelChunk::new(Dimension::Nether, ChunkPos::new(x, z))
    }

    /// Two areas with a gap between them, and the home chunk in the gap.
    fn gap() -> Division {
        Division {
            home: ChunkPos::new(0, 0),
            pinned: vec![area(None, Some(0)), area(Some(16), None)],
        }
    }

    /// The homes of the Nether and of the End in the table of [`gap`]: made after its
    /// three regions of the overworld, in that order.
    const NETHER_HOME: RegionId = RegionId(3);
    const END_HOME: RegionId = RegionId(4);

    /// The table of [`gap`], whose Nether home holds the Nether's chunk (0, 0) and has
    /// been granted one more, read from its file.
    fn with_the_nether() -> Table {
        let mut table = Table::made_from(&gap(), 0, 1);
        table.grant(NETHER_HOME, 6, &[n(1, 0)]).unwrap();
        Table::read(TableFile::decode(&table.file(1).encode()).unwrap()).unwrap()
    }

    #[test]
    fn regions_side_by_side_are_pinned_regions_numbered_from_west_to_east() {
        let cuts = [0, 4];
        let division = Division::side_by_side(ChunkPos::new(2, -7), &cuts).unwrap();
        division.check().unwrap();
        let table = Table::made_from(&division, 0, 1);
        // The three pinned regions, and then the homes of the Nether and the End.
        assert_eq!(table.next_region, 5);
        assert_eq!(table.home_region, RegionId(1));
        assert_eq!(
            (table.nether_home, table.end_home),
            (RegionId(3), RegionId(4))
        );
        let areas = [
            area(None, Some(0)),
            area(Some(0), Some(4)),
            area(Some(4), None),
        ];
        for (region, area) in (0..).map(RegionId).zip(areas) {
            assert_eq!(table.pinned(region), [area]);
            assert_eq!(table.dimension(region), Some(Dimension::Overworld));
        }
        for x in -40..40 {
            let chunk = o(x, x * 3);
            // A cut belongs to the region east of it.
            let holder = RegionId(cuts.partition_point(|cut| *cut <= x) as u32);
            assert_eq!(table.holder(chunk), Some(holder));
            for region in table.regions() {
                let held = (region == holder).then_some(0);
                assert_eq!(table.held_from(region, chunk), held);
            }
        }
        // One region pinned to the whole world.
        let single = Division::side_by_side(ChunkPos::new(0, 0), &[]).unwrap();
        let table = Table::made_from(&single, 0, 1);
        assert_eq!((table.home_region, table.next_region), (RegionId(0), 3));
        assert_eq!(table.pinned(RegionId(0)), [ChunkArea::EVERYWHERE]);
    }

    #[test]
    fn a_home_chunk_in_no_area_is_granted_to_a_region_made_after_the_pinned_ones() {
        let table = Table::made_from(&gap(), 0, 1);
        // The homes of the Nether and the End are made after it.
        assert_eq!((table.home_region, table.next_region), (RegionId(2), 5));
        assert_eq!(table.pinned(RegionId(2)), []);
        assert_eq!(table.holder(o(0, 0)), Some(RegionId(2)));
        assert_eq!(table.held_from(RegionId(2), o(0, 0)), Some(0));
        // The rest of the gap is nobody's.
        assert_eq!(table.holder(o(0, 1)), None);
        assert_eq!(table.holder(o(15, 0)), None);
        assert_eq!(table.holder(o(-1, 0)), Some(RegionId(0)));
        assert_eq!(table.holder(o(16, 0)), Some(RegionId(1)));
        assert_eq!(table.held_from(RegionId(1), o(0, 0)), None);
        assert_eq!(table.held_from(RegionId(9), o(0, 0)), None);

        // No pinned region at all: the world begins with the home region alone.
        let alone = Division {
            home: ChunkPos::new(3, 3),
            pinned: Vec::new(),
        };
        let table = Table::made_from(&alone, 0, 1);
        assert_eq!((table.home_region, table.next_region), (RegionId(0), 3));
        assert_eq!(table.holder(o(3, 3)), Some(RegionId(0)));
        // An id that the world has used before is not given to a region made later.
        assert_eq!(Table::made_from(&alone, 7, 1).next_region, 7);
    }

    #[test]
    fn an_open_world_is_one_home_region_that_is_pinned_to_nothing() {
        let home = ChunkPos::new(3, -7);
        let open = Division::open(home);
        assert_eq!((open.home, &open.pinned), (home, &vec![]));
        open.check().unwrap();
        let table = Table::made_from(&open, 0, 1);
        // Besides it, the homes of the Nether and the End, each holding its
        // dimension's chunk (0, 0).
        assert_eq!((table.home_region, table.next_region), (RegionId(0), 3));
        let (nether_home, end_home) = (RegionId(1), RegionId(2));
        assert_eq!((table.nether_home, table.end_home), (nether_home, end_home));
        assert_eq!(
            table.regions().collect::<Vec<_>>(),
            [RegionId(0), nether_home, end_home]
        );
        assert_eq!(table.pinned(RegionId(0)), []);
        assert_eq!(table.grants(RegionId(0)), [(o(3, -7), 0)]);
        assert_eq!(table.grants(nether_home), [(n(0, 0), 0)]);
        let end = LevelChunk::new(Dimension::End, ChunkPos::new(0, 0));
        assert_eq!(table.grants(end_home), [(end, 0)]);
        // Every other chunk is nobody's.
        for chunk in [o(3, -6), o(0, 0), n(3, -7)] {
            assert_eq!(table.holder(chunk), None);
        }
        let origin = ChunkPos::new(0, 0);
        let at_origin = ChunkBox {
            min: origin,
            max: origin,
        };
        let around = ChunkBox {
            min: home,
            max: home,
        };
        let expected = RegionList {
            homes: vec![
                (Dimension::Overworld, RegionId(0)),
                (Dimension::Nether, nether_home),
                (Dimension::End, end_home),
            ],
            spawn: crate::tests::SPAWN,
            home: RegionId(0),
            regions: vec![
                RegionInfo {
                    dimension: Dimension::Overworld,
                    region: RegionId(0),
                    epoch: 0,
                    bounds: Some(around),
                    pinned: Vec::new(),
                },
                RegionInfo {
                    dimension: Dimension::Nether,
                    region: nether_home,
                    epoch: 0,
                    bounds: Some(at_origin),
                    pinned: Vec::new(),
                },
                RegionInfo {
                    dimension: Dimension::End,
                    region: end_home,
                    epoch: 0,
                    bounds: Some(at_origin),
                    pinned: Vec::new(),
                },
            ],
            absorbed: Vec::new(),
            next: RegionId(3),
        };
        assert_eq!(table.list(crate::tests::SPAWN, |_| 0), expected);

        // The file has no area, and is read as the table it was written from.
        let file = table.file(table.from);
        assert_eq!(
            (&file.division, &file.regions[0].pinned),
            (&vec![], &vec![])
        );
        let read = Table::read(TableFile::decode(&file.encode()).unwrap()).unwrap();
        assert_eq!(read, table);
        assert!(read.is_of(&open));
        // Another home chunk is another world, and so is one region pinned to all of it.
        assert!(!read.is_of(&Division::open(ChunkPos::new(3, -6))));
        assert!(!read.is_of(&Division::side_by_side(home, &[]).unwrap()));
    }

    #[test]
    fn regions_side_by_side_are_cut_where_they_are_told_and_cover_the_world() {
        let area = |min_x, max_x| ChunkArea { min_x, max_x };
        let home = ChunkPos::new(2, -7);
        let three = Division::side_by_side(home, &[-2, 4]).unwrap();
        let areas = [
            area(None, Some(-2)),
            area(Some(-2), Some(4)),
            area(Some(4), None),
        ];
        assert_eq!((three.home, &three.pinned[..]), (home, &areas[..]));
        let one = Division::side_by_side(home, &[]).unwrap();
        assert_eq!(one.pinned, [ChunkArea::EVERYWHERE]);

        for cuts in [vec![], vec![4], vec![0, 4], vec![-2, 0, 5]] {
            for home in [ChunkPos::new(0, 0), ChunkPos::new(4, 1), home] {
                let pins = Division::side_by_side(home, &cuts).unwrap();
                pins.check().unwrap();
                assert_eq!(pins.home, home);
                // The areas are those a world of stripes at these cuts had: each from
                // its cut up to the next, and nothing between them.
                assert_eq!(pins.pinned.len(), cuts.len() + 1);
                for (index, area) in pins.pinned.iter().enumerate() {
                    let west = index.checked_sub(1).map(|west| cuts[west]);
                    let east = cuts.get(index).copied();
                    assert_eq!((area.min_x, area.max_x), (west, east), "{cuts:?}");
                }
                // The table is the one such a world had, so it is found as it was.
                let of_pins = Table::made_from(&pins, 0, 1);
                assert!(of_pins.is_of(&pins), "{cuts:?}");
                let home_region = cuts.partition_point(|cut| *cut <= home.x) as u32;
                assert_eq!(of_pins.home_region, RegionId(home_region));
            }
        }
    }

    #[test]
    fn cuts_that_do_not_ascend_are_no_division() {
        let home = ChunkPos::new(0, 0);
        for cuts in [&[4, 4][..], &[5, 4], &[0, 4, 4], &[-2, 0, -1], &[1, 0, 5]] {
            assert_eq!(Division::side_by_side(home, cuts), Err(NotAscending));
        }
        for cuts in [&[][..], &[4], &[-4, 4], &[i32::MIN, 0, i32::MAX]] {
            assert!(Division::side_by_side(home, cuts).is_ok(), "{cuts:?}");
        }
        // The command line refuses such coordinates in these words.
        assert_eq!(
            format!("--pin takes {NotAscending}"),
            "--pin takes chunk x coordinates in ascending order without repetitions"
        );
    }

    #[test]
    fn areas_that_overlap_are_no_division() {
        let overlapping = [
            vec![ChunkArea::EVERYWHERE, area(Some(3), Some(4))],
            vec![
                area(None, Some(1)),
                area(Some(5), None),
                area(Some(0), None),
            ],
            vec![area(Some(0), Some(5)), area(Some(4), Some(9))],
        ];
        for pinned in overlapping {
            let division = Division {
                home: ChunkPos::new(0, 0),
                pinned,
            };
            assert!(
                matches!(division.check(), Err(StoreError::Division { .. })),
                "{division:?}"
            );
        }
        // Areas that touch do not overlap: an area has its western end only.
        gap().check().unwrap();
        let touching = Division {
            pinned: vec![area(Some(0), Some(5)), area(Some(5), Some(9))],
            ..gap()
        };
        touching.check().unwrap();
    }

    #[test]
    fn the_table_is_what_its_file_says_and_the_file_what_the_table_is() {
        let table = Table::made_from(&gap(), 0, 4);
        let file = table.file(table.from);
        assert_eq!(file.from, 4);
        // The three regions of the overworld, and the homes of the Nether and the End,
        // each holding its dimension's chunk (0, 0).
        assert_eq!(file.regions.len(), 5);
        assert_eq!(file.regions[2].grants, [(ChunkPos::new(0, 0), 0)]);
        assert_eq!((file.nether_home, file.end_home), (3, 4));
        let dimensions: Vec<_> = file.regions.iter().map(|region| region.dimension).collect();
        let (overworld, others) = dimensions.split_at(3);
        assert!(overworld.iter().all(|d| *d == Dimension::Overworld));
        assert_eq!(others, [Dimension::Nether, Dimension::End]);
        for home in &file.regions[3..] {
            assert_eq!(home.grants, [(ChunkPos::new(0, 0), 0)]);
        }
        let read = Table::read(TableFile::decode(&file.encode()).unwrap()).unwrap();
        assert_eq!(read, table);
        assert!(read.is_of(&gap()));
        // Another home chunk is another division, and so are other areas.
        let moved = Division {
            home: ChunkPos::new(1, 0),
            ..gap()
        };
        assert!(!read.is_of(&moved));
        let narrower = Division {
            pinned: vec![area(None, Some(0)), area(Some(17), None)],
            ..gap()
        };
        assert!(!read.is_of(&narrower));
    }

    #[test]
    fn a_table_file_that_says_what_cannot_be_is_refused() {
        let good = Table::made_from(&gap(), 0, 4).file(4);
        let refused = |change: fn(&mut TableFile)| {
            let mut file = good.clone();
            change(&mut file);
            let read = Table::read(file);
            assert!(matches!(read, Err(StoreError::Table(_))), "{read:?}");
        };
        // A chunk granted twice, a home region that does not hold the home chunk or
        // is none, a region whose id the next region would get again, two regions
        // pinned to the same chunks, and a region of another dimension than the
        // overworld that is pinned.
        refused(|file| file.regions[1].grants = vec![(ChunkPos::new(0, 0), 3)]);
        refused(|file| file.home_region = 1);
        refused(|file| file.home_region = 9);
        refused(|file| file.next_region = 2);
        refused(|file| file.regions[2].pinned = vec![ChunkArea::EVERYWHERE]);
        refused(|file| file.regions[0].dimension = Dimension::End);
        // The home region of another dimension than the overworld's does not hold the
        // overworld's home chunk, whatever position it holds.
        refused(|file| file.regions[2].dimension = Dimension::Nether);
        // Nor do the homes of the Nether and the End hold each other's chunk, or the
        // overworld's, and a home that is none holds nothing.
        refused(|file| file.nether_home = 4);
        refused(|file| file.end_home = 2);
        refused(|file| file.nether_home = 9);
    }

    #[test]
    fn chunks_are_granted_once_and_free_again_when_released() {
        let mut table = Table::made_from(&gap(), 0, 1);
        let (first, second) = (o(3, 3), o(4, -3));
        table.grant(RegionId(0), 7, &[first, second]).unwrap();
        assert_eq!(table.holder(first), Some(RegionId(0)));
        assert_eq!(table.held_from(RegionId(0), first), Some(7));
        assert_eq!(table.granted_from(RegionId(0), second), Some(7));
        assert_eq!(table.grants(RegionId(0)), [(first, 7), (second, 7)]);
        assert_eq!(table.held_from(RegionId(1), first), None);
        // A chunk held by being pinned is not granted.
        assert_eq!(table.granted_from(RegionId(0), o(-1, 0)), None);
        assert_eq!(table.held_from(RegionId(0), o(-1, 0)), Some(0));

        // Granted already, to another region or the same; or to no region at all. None
        // of the chunks of such a grant is granted.
        let before = table.clone();
        let free = o(9, 9);
        for (region, chunks) in [(1, [free, first]), (0, [free, second]), (7, [free, free])] {
            let granted = table.grant(RegionId(region), 9, &chunks);
            assert!(matches!(granted, Err(StoreError::Table(_))), "{granted:?}");
            assert_eq!(table, before);
        }

        // Only the region that has the grant gives it up, and only once.
        assert_eq!(table.release(RegionId(1), first), None);
        assert_eq!(table.release(RegionId(9), first), None);
        assert_eq!(table.release(RegionId(0), first), Some(7));
        assert_eq!(table.release(RegionId(0), first), None);
        assert_eq!(table.holder(first), None);
        table.grant(RegionId(1), 2, &[first]).unwrap();
        assert_eq!(table.holder(first), Some(RegionId(1)));

        // A chunk of a pinned region's area that another region was granted is that
        // region's, and the pinned region's again once it is released.
        let inside = o(-4, 0);
        table.grant(RegionId(2), 5, &[inside]).unwrap();
        assert_eq!(table.holder(inside), Some(RegionId(2)));
        assert_eq!(table.held_from(RegionId(0), inside), None);
        assert_eq!(table.release(RegionId(2), inside), Some(5));
        assert_eq!(table.held_from(RegionId(0), inside), Some(0));

        // The file has the grants, and the list the box around them.
        let read = Table::read(TableFile::decode(&table.file(3).encode()).unwrap()).unwrap();
        assert_eq!(read.from, 3);
        assert_eq!(read.grants(RegionId(0)), [(second, 7)]);
        assert_eq!(read.holder(first), Some(RegionId(1)));
        let around = ChunkBox {
            min: second.position,
            max: second.position,
        };
        assert_eq!(
            read.list(crate::tests::SPAWN, |_| 0).regions[0].bounds,
            Some(around)
        );
    }

    #[test]
    fn a_merge_gives_the_survivor_what_the_absorbed_region_held() {
        let mut table = Table::made_from(&gap(), 0, 1);
        let (first, second) = (o(3, 3), o(4, -3));
        table.grant(RegionId(1), 7, &[second, first]).unwrap();
        table.grant(RegionId(0), 2, &[o(9, 9)]).unwrap();

        // What cannot be: with itself, with or by a region that is none, and of a home,
        // the overworld's or another dimension's.
        let before = table.clone();
        for (region, absorbed) in [(0, 0), (0, 9), (9, 1), (0, 2), (3, 3), (3, 4)] {
            let merged = table.absorb(RegionId(region), RegionId(absorbed), 9);
            assert!(matches!(merged, Err(StoreError::Table(_))), "{merged:?}");
            assert_eq!(table, before);
        }

        assert_eq!(
            table.absorb(RegionId(0), RegionId(1), 9).unwrap(),
            [first, second]
        );
        assert!(!table.has(RegionId(1)));
        assert_eq!(table.absorbed_into(RegionId(1)), Some(RegionId(0)));
        assert_eq!(table.absorbed_into(RegionId(0)), None);
        // The survivor is pinned to both areas, and holds the chunks from the merge on;
        // what it held before, it holds from when it did.
        assert_eq!(table.pinned(RegionId(0)), gap().pinned);
        assert_eq!(
            table.grants(RegionId(0)),
            [(first, 9), (second, 9), (o(9, 9), 2)]
        );
        assert_eq!(table.holder(o(20, 0)), Some(RegionId(0)));
        assert_eq!(table.held_from(RegionId(0), o(20, 0)), Some(0));
        // Its id is not used again, and it is still the division it was made from.
        assert_eq!(table.next_region, 5);
        assert!(table.is_of(&gap()));
        let list = table.list(crate::tests::SPAWN, |_| 0);
        assert_eq!(list.absorbed, [(RegionId(1), RegionId(0))]);
        let listed = list.regions.iter().map(|info| info.region);
        assert_eq!(
            listed.collect::<Vec<_>>(),
            [RegionId(0), RegionId(2), NETHER_HOME, END_HOME]
        );
        let read = Table::read(TableFile::decode(&table.file(1).encode()).unwrap()).unwrap();
        assert_eq!(read, table);

        // The home region absorbs as any other, and of those absorbed only so many are
        // remembered, the latest.
        assert_eq!(table.absorb(RegionId(2), RegionId(0), 10).unwrap().len(), 3);
        assert_eq!(table.absorbed_into(RegionId(0)), Some(RegionId(2)));
        for part in 5..5 + ABSORBED_KEPT as u32 {
            table
                .split(RegionId(2), RegionId(part), 11, &[first])
                .unwrap();
            table.absorb(RegionId(2), RegionId(part), 12).unwrap();
        }
        assert_eq!(
            table.list(crate::tests::SPAWN, |_| 0).absorbed.len(),
            ABSORBED_KEPT
        );
        assert_eq!(table.absorbed_into(RegionId(1)), None);
        assert_eq!(table.absorbed_into(RegionId(5)), Some(RegionId(2)));
    }

    #[test]
    fn a_split_gives_the_part_its_chunks_and_an_id_of_its_own() {
        let mut table = Table::made_from(&gap(), 0, 1);
        let granted = o(3, 3);
        let pinned = o(-4, 0);
        table.grant(RegionId(0), 7, &[granted]).unwrap();

        // What cannot be: an id that has been used (a home's among them), a region that
        // is none, and a chunk the region does not hold.
        let before = table.clone();
        let origin = o(0, 0);
        for (region, part, chunk) in [
            (0, 2, granted),
            (0, 1, granted),
            (0, 4, granted),
            (9, 5, granted),
            (0, 5, origin),
        ] {
            let split = table.split(RegionId(region), RegionId(part), 9, &[granted, chunk]);
            assert!(matches!(split, Err(StoreError::Table(_))), "{split:?}");
            assert_eq!(table, before);
        }

        // A chunk the region was granted and one it holds by being pinned.
        table
            .split(RegionId(0), RegionId(5), 9, &[pinned, granted])
            .unwrap();
        assert_eq!(table.next_region, 6);
        assert_eq!(table.pinned(RegionId(5)), []);
        assert_eq!(table.grants(RegionId(5)), [(pinned, 9), (granted, 9)]);
        assert_eq!(table.grants(RegionId(0)), []);
        for chunk in [pinned, granted] {
            assert_eq!(table.holder(chunk), Some(RegionId(5)));
            assert_eq!(table.held_from(RegionId(0), chunk), None);
        }
        // The rest of the area is the pinned region's still, and the chunk that was
        // split off is its own again, from tick 0, once it is given back.
        assert_eq!(table.held_from(RegionId(0), o(-4, 1)), Some(0));
        assert_eq!(table.release(RegionId(5), pinned), Some(9));
        assert_eq!(table.held_from(RegionId(0), pinned), Some(0));
        let read = Table::read(TableFile::decode(&table.file(1).encode()).unwrap()).unwrap();
        assert_eq!(read, table);
        // A later id than the next is taken as it is, and none below it is used after.
        table.split(RegionId(0), RegionId(8), 9, &[pinned]).unwrap();
        assert_eq!(table.next_region, 9);
    }

    #[test]
    fn the_list_has_the_regions_with_what_they_are_pinned_to() {
        let table = Table::made_from(&gap(), 0, 1);
        let list = table.list(crate::tests::SPAWN, |region| u64::from(region.0) * 10);
        assert_eq!(list.home, RegionId(2));
        assert_eq!(
            list.homes,
            [
                (Dimension::Overworld, RegionId(2)),
                (Dimension::Nether, NETHER_HOME),
                (Dimension::End, END_HOME),
            ]
        );
        assert_eq!(list.absorbed, []);
        let origin = ChunkPos::new(0, 0);
        let expected = [
            (0, vec![area(None, Some(0))], None),
            (1, vec![area(Some(16), None)], None),
            (
                2,
                Vec::new(),
                Some(ChunkBox {
                    min: origin,
                    max: origin,
                }),
            ),
            // The homes of the Nether and the End, each at its dimension's origin.
            (
                3,
                Vec::new(),
                Some(ChunkBox {
                    min: origin,
                    max: origin,
                }),
            ),
            (
                4,
                Vec::new(),
                Some(ChunkBox {
                    min: origin,
                    max: origin,
                }),
            ),
        ]
        .map(|(region, pinned, bounds)| RegionInfo {
            dimension: match region {
                3 => Dimension::Nether,
                4 => Dimension::End,
                _ => Dimension::Overworld,
            },
            region: RegionId(region),
            epoch: u64::from(region) * 10,
            bounds,
            pinned,
        });
        assert_eq!(list.regions, expected);

        let chunks = [(3, -2), (-1, 5), (0, 0)].map(|(x, z)| ChunkPos::new(x, z));
        let around = ChunkBox {
            min: ChunkPos::new(-1, -2),
            max: ChunkPos::new(3, 5),
        };
        assert_eq!(bounds(chunks.into_iter()), Some(around));
        assert_eq!(bounds(std::iter::empty()), None);
    }

    #[test]
    fn a_region_keeps_its_dimension_in_the_file_the_list_and_its_split_parts() {
        let mut table = with_the_nether();
        assert_eq!(table.dimension(NETHER_HOME), Some(Dimension::Nether));
        assert_eq!(table.grants(NETHER_HOME), [(n(0, 0), 0), (n(1, 0), 6)]);
        let list = table.list(crate::tests::SPAWN, |_| 0);
        let info = list.regions.iter().find(|info| info.region == NETHER_HOME);
        let info = info.expect("the Nether's region is listed");
        assert_eq!(info.dimension, Dimension::Nether);
        let origin = ChunkPos::new(0, 0);
        let east = ChunkPos::new(1, 0);
        assert_eq!(
            info.bounds,
            Some(ChunkBox {
                min: origin,
                max: east
            })
        );
        // Its file names the dimension, and its grants by their positions.
        let file = table.file(1);
        assert_eq!(file.regions[3].id, NETHER_HOME.0);
        assert_eq!(file.regions[3].dimension, Dimension::Nether);
        assert_eq!(file.regions[3].grants, [(origin, 0), (east, 6)]);
        // A split's part is of its region's dimension.
        let part = RegionId(table.next_region);
        table.split(NETHER_HOME, part, 8, &[n(1, 0)]).unwrap();
        assert_eq!(table.dimension(part), Some(Dimension::Nether));
        assert_eq!(table.holder(n(1, 0)), Some(part));
        let read = Table::read(TableFile::decode(&table.file(1).encode()).unwrap()).unwrap();
        assert_eq!(read, table);
        assert_eq!(read.dimension(part), Some(Dimension::Nether));
    }

    #[test]
    fn the_same_position_in_two_dimensions_is_two_chunks_and_pins_are_the_overworld_s() {
        let mut table = with_the_nether();
        // The overworld's (0, 0) is the home region's, the Nether's its home's.
        assert_eq!(table.holder(o(0, 0)), Some(RegionId(2)));
        assert_eq!(table.holder(n(0, 0)), Some(NETHER_HOME));
        assert_eq!(table.held_from(RegionId(2), n(0, 0)), None);
        assert_eq!(table.held_from(NETHER_HOME, o(0, 0)), None);
        // Within a pinned area of the overworld, a Nether chunk is nobody's: neither
        // `holder` nor `pinned_holder` nor `held_from` looks at areas outside the
        // overworld (`docs/groundwork/p-plan.md`, section 3.3, R D3).
        assert_eq!(table.holder(o(-5, 2)), Some(RegionId(0)));
        assert_eq!(table.pinned_holder(o(-5, 2)), Some(RegionId(0)));
        assert_eq!(table.holder(n(-5, 2)), None);
        assert_eq!(table.pinned_holder(n(-5, 2)), None);
        assert_eq!(table.held_from(RegionId(0), n(-5, 2)), None);
        // A Nether chunk is granted to a Nether region, and to no region of another
        // dimension, which would take it for one of its own.
        let before = table.clone();
        let granted = table.grant(RegionId(2), 9, &[n(-5, 2)]);
        assert!(matches!(granted, Err(StoreError::Table(_))), "{granted:?}");
        let granted = table.grant(NETHER_HOME, 9, &[o(5, 5)]);
        assert!(matches!(granted, Err(StoreError::Table(_))), "{granted:?}");
        assert_eq!(table, before);
        table.grant(NETHER_HOME, 9, &[n(-5, 2)]).unwrap();
        assert_eq!(table.holder(n(-5, 2)), Some(NETHER_HOME));
        // Nothing of the Nether is lent or borrowed: lends are at the overworld's pins.
        assert_eq!(table.lends(RegionId(0)), (Vec::new(), Vec::new()));
        assert_eq!(table.lends(NETHER_HOME), (Vec::new(), Vec::new()));
    }

    #[test]
    fn no_merge_joins_two_dimensions() {
        let mut table = with_the_nether();
        // A region of the Nether that is not its home, and so may be absorbed.
        let part = RegionId(table.next_region);
        table.split(NETHER_HOME, part, 8, &[n(1, 0)]).unwrap();
        let before = table.clone();
        for (region, absorbed) in [(RegionId(0), part), (part, RegionId(1))] {
            let merged = table.absorb(region, absorbed, 9);
            assert!(matches!(merged, Err(StoreError::Table(_))), "{merged:?}");
            assert_eq!(table, before);
        }
        // Within the Nether a merge is as any other, and an absorbed region is of the
        // dimension of the region it went into.
        assert_eq!(table.absorb(NETHER_HOME, part, 9).unwrap(), [n(1, 0)]);
        assert_eq!(table.dimension(part), None);
        assert_eq!(table.dimension_ever(part), Some(Dimension::Nether));
        assert_eq!(
            table.dimension_ever(RegionId(0)),
            Some(Dimension::Overworld)
        );
        assert_eq!(table.dimension_ever(RegionId(99)), None);
    }

    #[test]
    fn each_dimension_has_a_home_chunk_the_overworld_s_the_spawn_s() {
        let table = Table::made_from(&Division::open(ChunkPos::new(3, -7)), 0, 1);
        assert_eq!(table.home(), o(3, -7));
        assert_eq!(table.home_of(Dimension::Overworld), o(3, -7));
        assert_eq!(table.home_of(Dimension::Nether), n(0, 0));
        let end = LevelChunk::new(Dimension::End, ChunkPos::new(0, 0));
        assert_eq!(table.home_of(Dimension::End), end);
        assert!(table.is_home(o(3, -7)) && table.is_home(n(0, 0)) && table.is_home(end));
        assert!(!table.is_home(o(0, 0)) && !table.is_home(n(3, -7)));
    }
}
