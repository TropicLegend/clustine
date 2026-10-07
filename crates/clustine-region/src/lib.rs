//! Region model: how the world is divided into regions and who runs each.
//!
//! For now the division is fixed when a world's cluster is started: the world is cut
//! into stripes along the x axis. Regions that follow where players are, and merge and
//! split as they move, will replace this.

use clustine_world::{ChunkArea, ChunkPos, Vec3};
use serde::{Deserialize, Serialize};

/// Identifies a region within a [`Layout`]: regions are numbered from west to east.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u32);

impl std::fmt::Display for RegionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Why a list of boundaries does not describe a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("region boundaries must be in ascending order without repetitions")]
pub struct LayoutError;

/// How the world is divided into regions: stripes along the x axis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// The chunk x coordinates at which a region ends and the next begins, ascending.
    /// The chunk with that x coordinate belongs to the region east of the boundary.
    boundaries: Vec<i32>,
}

impl Layout {
    /// The whole world as one region.
    pub fn single() -> Self {
        Self {
            boundaries: Vec::new(),
        }
    }

    /// A layout with a region boundary at each of the given chunk x coordinates.
    pub fn new(boundaries: Vec<i32>) -> Result<Self, LayoutError> {
        if boundaries.windows(2).all(|pair| pair[0] < pair[1]) {
            Ok(Self { boundaries })
        } else {
            Err(LayoutError)
        }
    }

    pub fn boundaries(&self) -> &[i32] {
        &self.boundaries
    }

    pub fn region_count(&self) -> usize {
        self.boundaries.len() + 1
    }

    /// All regions with the part of the world each covers, from west to east.
    pub fn regions(&self) -> impl Iterator<Item = (RegionId, ChunkArea)> + '_ {
        (0..self.region_count()).map(|index| {
            let area = ChunkArea {
                min_x: index.checked_sub(1).map(|west| self.boundaries[west]),
                max_x: self.boundaries.get(index).copied(),
            };
            (RegionId(index as u32), area)
        })
    }

    /// The part of the world `region` covers, if the layout has such a region.
    pub fn area(&self, region: RegionId) -> Option<ChunkArea> {
        self.regions().nth(region.0 as usize).map(|(_, area)| area)
    }

    /// The region `chunk` belongs to.
    pub fn region_of(&self, chunk: ChunkPos) -> RegionId {
        RegionId(
            self.boundaries
                .partition_point(|boundary| *boundary <= chunk.x) as u32,
        )
    }

    /// A number that is the same for equal layouts and, in practice, different for
    /// different ones, in every process and version. Services compare it to make sure
    /// they divide the world the same way.
    pub fn fingerprint(&self) -> u64 {
        // FNV-1a.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let count = (self.boundaries.len() as u32).to_be_bytes();
        let coordinates = self.boundaries.iter().flat_map(|x| x.to_be_bytes());
        for byte in count.into_iter().chain(coordinates) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

/// Where an edge reaches the worker that runs a region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionRoute {
    pub region: RegionId,
    /// Counts the owners the region has had. An owner with a lower epoch than another
    /// has been replaced.
    pub epoch: u64,
    /// Host and port of the worker.
    pub address: String,
}

/// What an edge needs to know about the cluster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingTable {
    /// Increases whenever the table changes.
    pub version: u64,
    pub layout: Layout,
    /// Where players enter the world.
    pub spawn: Vec3,
    /// The regions that have an owner, in ascending order of their ids.
    pub routes: Vec<RegionRoute>,
}

impl RoutingTable {
    /// Whether every region has an owner.
    pub fn is_complete(&self) -> bool {
        self.routes.len() == self.layout.region_count()
    }

    pub fn route(&self, region: RegionId) -> Option<&RegionRoute> {
        self.routes.iter().find(|route| route.region == region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn areas(layout: &Layout) -> Vec<(Option<i32>, Option<i32>)> {
        layout
            .regions()
            .map(|(_, area)| (area.min_x, area.max_x))
            .collect()
    }

    #[test]
    fn a_single_region_covers_everything() {
        let layout = Layout::single();
        assert_eq!(layout.region_count(), 1);
        assert_eq!(areas(&layout), [(None, None)]);
        assert_eq!(layout.region_of(ChunkPos::new(i32::MIN, 7)), RegionId(0));
        assert_eq!(layout.region_of(ChunkPos::new(i32::MAX, -7)), RegionId(0));
        assert_eq!(layout.area(RegionId(0)), Some(ChunkArea::EVERYWHERE));
        assert_eq!(layout.area(RegionId(1)), None);
    }

    #[test]
    fn boundaries_belong_to_the_region_east_of_them() {
        let layout = Layout::new(vec![-4, 0, 10]).unwrap();
        assert_eq!(layout.region_count(), 4);
        assert_eq!(
            areas(&layout),
            [
                (None, Some(-4)),
                (Some(-4), Some(0)),
                (Some(0), Some(10)),
                (Some(10), None),
            ]
        );
        for (x, region) in [(-5, 0), (-4, 1), (-1, 1), (0, 2), (9, 2), (10, 3), (99, 3)] {
            assert_eq!(layout.region_of(ChunkPos::new(x, 3)), RegionId(region));
        }
    }

    /// Whatever `region_of` says, the area of that region agrees.
    #[test]
    fn every_chunk_is_in_exactly_the_region_it_belongs_to() {
        let layout = Layout::new(vec![-2, 1]).unwrap();
        for x in -6..6 {
            let chunk = ChunkPos::new(x, x * 3);
            let owner = layout.region_of(chunk);
            for (region, area) in layout.regions() {
                assert_eq!(area.contains(chunk), region == owner, "{chunk:?}");
            }
        }
    }

    #[test]
    fn boundaries_must_ascend() {
        assert!(Layout::new(vec![]).is_ok());
        assert!(Layout::new(vec![5]).is_ok());
        assert_eq!(Layout::new(vec![1, 1]), Err(LayoutError));
        assert_eq!(Layout::new(vec![2, 1]), Err(LayoutError));
    }

    #[test]
    fn fingerprints_tell_layouts_apart() {
        let fingerprint =
            |boundaries: &[i32]| Layout::new(boundaries.to_vec()).unwrap().fingerprint();
        assert_eq!(fingerprint(&[0]), fingerprint(&[0]));
        assert_ne!(fingerprint(&[]), fingerprint(&[0]));
        assert_ne!(fingerprint(&[0]), fingerprint(&[1]));
        assert_ne!(fingerprint(&[0, 256]), fingerprint(&[256]));
        // Pinned: other processes, possibly of another version, compare against it.
        assert_eq!(Layout::single().fingerprint(), 0x4d25_767f_9dce_13f5);
    }

    #[test]
    fn a_routing_table_is_complete_when_every_region_has_a_route() {
        let route = |region| RegionRoute {
            region: RegionId(region),
            epoch: 1,
            address: format!("worker-{region}:25601"),
        };
        let mut table = RoutingTable {
            version: 1,
            layout: Layout::new(vec![0]).unwrap(),
            spawn: Vec3::new(0.5, -60.0, 0.5),
            routes: vec![route(1)],
        };
        assert!(!table.is_complete());
        assert_eq!(table.route(RegionId(0)), None);
        table.routes.insert(0, route(0));
        assert!(table.is_complete());
        assert_eq!(table.route(RegionId(1)).unwrap().address, "worker-1:25601");
    }
}
