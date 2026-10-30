//! The list of regions as the overworld's tests read it.
//!
//! Every world is made with a home region for the Nether and one for the End besides
//! the overworld's regions (`docs/groundwork/p-plan.md`, decision 2). Nothing in the
//! tests of the overworld goes to either, so those two are neither split nor merged
//! (they may change hands, as every region does); the tests say what they mean of the
//! overworld's regions by these, and check that the other homes are as the world made
//! them.

use clustine_region::RegionId;
use clustine_rpc::{RegionInfo, RegionList};
use clustine_world::{ChunkPos, Dimension};

/// How many regions besides the overworld's every world has: the homes of the Nether
/// and of the End. A cluster that is to have a worker for every region counts them.
pub const OTHER_HOMES: usize = 2;

/// What `list` has of the overworld's regions, in the order of their ids.
pub fn overworld(list: &RegionList) -> Vec<&RegionInfo> {
    let regions = list.regions.iter();
    regions
        .filter(|info| info.dimension == Dimension::Overworld)
        .collect()
}

/// `list` as it would be of the overworld alone: without the homes of the Nether
/// and of the End, which nothing absorbed and which absorbed nothing.
pub fn overworld_list(list: &RegionList) -> RegionList {
    RegionList {
        regions: overworld(list).into_iter().cloned().collect(),
        ..list.clone()
    }
}

/// The ids of the overworld's regions in `list`, in ascending order.
pub fn overworld_ids(list: &RegionList) -> Vec<RegionId> {
    overworld(list).iter().map(|info| info.region).collect()
}

/// The homes of the Nether and of the End by `list`, in that order.
pub fn other_homes(list: &RegionList) -> Vec<RegionId> {
    let homes = list.homes.iter();
    homes
        .filter(|(dimension, _)| *dimension != Dimension::Overworld)
        .map(|(_, region)| *region)
        .collect()
}

/// Whether `region` is the home of the Nether or of the End by `list`.
pub fn is_other_home(list: &RegionList, region: RegionId) -> bool {
    other_homes(list).contains(&region)
}

/// Whether the homes of the Nether and of the End are as a world is made with them,
/// and nothing that was done to the overworld touched them: each lives, of its own
/// dimension, holding its chunk (0, 0) alone, absorbed by nothing and absorbing
/// nothing; and no other region is of a dimension but the overworld. Says what is
/// not so.
pub fn other_homes_are_untouched(list: &RegionList) -> Result<(), String> {
    let origin = ChunkPos::new(0, 0);
    for dimension in [Dimension::Nether, Dimension::End] {
        let home = list.homes.iter().find(|(of, _)| *of == dimension);
        let Some((_, home)) = home else {
            return Err(format!("the list names no home of {dimension:?}: {list:?}"));
        };
        let Some(info) = list.regions.iter().find(|info| info.region == *home) else {
            return Err(format!("the home of {dimension:?} does not live: {list:?}"));
        };
        let alone = info
            .bounds
            .is_some_and(|bounds| bounds.min == origin && bounds.max == origin);
        if info.dimension != dimension || !alone || !info.pinned.is_empty() {
            return Err(format!(
                "the home of {dimension:?} is not as made: {list:?}"
            ));
        }
        let merged = |(gone, into): &(RegionId, RegionId)| gone == home || into == home;
        if list.absorbed.iter().any(merged) {
            return Err(format!(
                "the home of {dimension:?} was in a merge: {list:?}"
            ));
        }
    }
    let mut others = list
        .regions
        .iter()
        .filter(|info| info.dimension != Dimension::Overworld && !is_other_home(list, info.region));
    if let Some(stray) = others.next() {
        return Err(format!(
            "region {} is of {:?} and no home: {list:?}",
            stray.region, stray.dimension
        ));
    }
    Ok(())
}

/// Panics unless [`other_homes_are_untouched`].
pub fn assert_other_homes_untouched(list: &RegionList) {
    if let Err(error) = other_homes_are_untouched(list) {
        panic!("{error}");
    }
}
