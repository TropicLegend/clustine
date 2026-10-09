//! Region model: which regions a world has and who runs each.
//!
//! Nothing here says how a world is divided. The world store alone knows which chunks
//! a region holds: regions follow where their players are, and merge and split as they
//! move (`docs/adr/0017-the-end-of-the-stripes.md`).

pub use clustine_world::RegionId;
use clustine_world::Vec3;
use serde::{Deserialize, Serialize};

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
    /// Where players enter the world.
    pub spawn: Vec3,
    /// The regions that have an owner, in ascending order of their ids.
    pub routes: Vec<RegionRoute>,
    /// The region players enter the world in, as the world store's list of regions had
    /// it when the coordinator last read it
    /// (`docs/adr/0014-merging-and-splitting.md`, section 5.5). `None` until it has.
    pub home: Option<RegionId>,
    /// The regions that were absorbed, each with the region it went into, which may
    /// have been absorbed since: all the world store keeps of them, as of the same
    /// reading. An edge acts on what a region tells it of a merge, and uses these to
    /// know which region to expect that from (ADR-0014, rule 39).
    pub absorbed: Vec<(RegionId, RegionId)>,
    /// How many regions the coordinator knows that have no owner. The regions merge
    /// and split, so nothing but the coordinator's count says whether `routes` is all
    /// of them.
    pub waiting: u32,
}

impl RoutingTable {
    /// Whether every region the coordinator knows has an owner.
    pub fn is_complete(&self) -> bool {
        self.waiting == 0
    }

    pub fn route(&self, region: RegionId) -> Option<&RegionRoute> {
        self.routes.iter().find(|route| route.region == region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_routing_table_is_complete_when_no_region_waits_for_an_owner() {
        let route = |region| RegionRoute {
            region: RegionId(region),
            epoch: 1,
            address: format!("worker-{region}:25601"),
        };
        let mut table = RoutingTable {
            home: None,
            absorbed: Vec::new(),
            waiting: 1,
            version: 1,
            spawn: Vec3::new(0.5, -60.0, 0.5),
            routes: vec![route(1)],
        };
        assert!(!table.is_complete());
        assert_eq!(table.route(RegionId(0)), None);
        table.routes.insert(0, route(0));
        table.waiting = 0;
        assert!(table.is_complete());
        assert_eq!(table.route(RegionId(1)).unwrap().address, "worker-1:25601");

        // The number of routes does not come into it: regions are split off and
        // absorbed, and only the coordinator's count says whether one waits.
        table.routes.push(route(2));
        assert!(table.is_complete());
        table.routes.truncate(1);
        assert!(table.is_complete());
        table.waiting = 2;
        assert!(!table.is_complete());
    }
}
