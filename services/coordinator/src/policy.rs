//! What a coordinator goes by that merges and splits regions by itself, and what it
//! wants by that. See `docs/adr/0016-when-to-merge-and-split.md`, sections 3, 4 and 8.
//!
//! Distances are counted in chunks along the longer of the two axes, as the region
//! counts them when it is split.
//!
//! [`decide`] and [`named`] are functions of what they are handed and of nothing
//! else: no clock, no map that is not ordered, and nothing kept from one call to the
//! next. Since when a thing has been wanted, and whether it may be begun, is for
//! whoever calls them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use clustine_rpc::Crowds;
use clustine_world::{ChunkPos, RegionId};

/// The distances by which regions are merged and split, and how long a region is left
/// alone after either.
///
/// Regions are merged before the views of their players touch, and a region is split
/// only when its players are a good deal further apart than that, so that a split is
/// not undone by a merge at the next step somebody takes. Whoever makes a policy of
/// distances it was told has it [`Policy::checked`] before anything goes by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Regions with players this near to each other or nearer are merged.
    pub merge_distance: u32,
    /// Players of one region that are further apart than this are split.
    pub split_distance: u32,
    /// How long a region is left alone after a merge, a split or a change of owner.
    pub rest: Duration,
}

impl Policy {
    /// The policy for edges that grant a view distance of `view_distance` at most,
    /// with a rest of ten seconds.
    ///
    /// A player is sent the chunks up to one more than the view distance away, so two
    /// players whose views do not touch are twice that and one apart or more. Regions
    /// merge four chunks before that, which is what two players in creative flight
    /// cover towards each other until a merge is done, and split eight chunks beyond.
    /// So the distances are `2 * view_distance + 6` and `2 * view_distance + 14`.
    ///
    /// Edges grant 32 at most. A view distance whose distances do not fit 32 bits
    /// gives the largest there are instead of overflowing.
    pub fn for_view_distance(view_distance: u32) -> Self {
        let merge_distance = view_distance.saturating_mul(2).saturating_add(6);
        Self {
            merge_distance,
            split_distance: merge_distance.saturating_add(8),
            rest: Duration::from_secs(10),
        }
    }

    /// The policy, if its distances fit each other, or why they do not.
    ///
    /// The merge distance has to be 1 at least, and the split distance 2 more than
    /// the merge distance at least. Then the split distance is 3 at least, the
    /// [`Policy::margin`] is 1 at least and twice the margin is less than the split
    /// distance. Nothing is asked of the rest.
    pub fn checked(self) -> Result<Self, String> {
        let Self {
            merge_distance,
            split_distance,
            ..
        } = self;
        if merge_distance < 1 {
            return Err("the merge distance has to be 1 at least".to_owned());
        }
        // In 64 bits, so that no distance is too large to be compared.
        if u64::from(merge_distance) + 2 > u64::from(split_distance) {
            return Err(format!(
                "the split distance has to be at least 2 more than the merge distance, \
                 and {split_distance} is not 2 more than {merge_distance}"
            ));
        }
        Ok(self)
    }

    /// How far around the chunks of a group a split names chunks, and how far a
    /// group may move in a tick and still be the same group: 3, or less where the
    /// split distance leaves no room for that.
    pub fn margin(&self) -> u32 {
        (self.split_distance.saturating_sub(1) / 2).min(3)
    }
}

/// A region as the coordinator last heard of it: what [`decide`] is told of every
/// region that has a sighting.
#[derive(Debug, Clone, Copy)]
pub struct Sighted<'a> {
    pub region: RegionId,
    /// Whether the sighting is fresh. Only a region whose sighting is fresh is merged
    /// or split; the players of any other still hold back what they may be part of.
    pub fresh: bool,
    /// The chunks with players in them, each with how many.
    pub crowds: &'a Crowds,
}

/// A merge or a split that is wanted by where the players are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// Players of the two regions are within the merge distance of each other, and
    /// `gap` is how far the nearest of them are apart.
    Merge {
        survivor: RegionId,
        absorbed: RegionId,
        gap: u32,
        why: Why,
    },
    /// The players of the region are in several clusters. `groups`: the groups that
    /// go, each the chunks its players are in, ascending, and the groups in the order
    /// of their lowest chunks.
    Split {
        region: RegionId,
        groups: Vec<Vec<ChunkPos>>,
        why: Why,
    },
}

/// Why a merge or a split is wanted. A later rule that wants one for a further reason
/// is a further case here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Players of two regions are near each other.
    Near,
    /// Players of one region are far apart, with nobody between them.
    Apart,
}

/// A region and a chunk in which its sighting has players; or the chunk players enter
/// the world in, which is a place of the home region whoever is there.
struct Place {
    region: RegionId,
    chunk: ChunkPos,
    /// How many players the sighting has there.
    players: u64,
    /// Whether the region's sighting is fresh.
    fresh: bool,
    /// Whether the place is known for sure: it is of a fresh sighting, or it is the
    /// chunk players enter in.
    known: bool,
}

/// How far two chunks are apart: in chunks along the longer of the two axes. In 64
/// bits, as the difference of two coordinates does not fit into 32. `Region::split`
/// counts the same, and a split is worked out by that.
pub(crate) fn distance(from: ChunkPos, to: ChunkPos) -> u64 {
    let along = |from: i32, to: i32| (i64::from(from) - i64::from(to)).unsigned_abs();
    along(from.x, to.x).max(along(from.z, to.z))
}

/// Whether two places hold together: players of one region up to the split distance,
/// and players of two regions up to the merge distance.
fn linked(policy: &Policy, one: &Place, other: &Place) -> bool {
    let reach = if one.region == other.region {
        policy.split_distance
    } else {
        policy.merge_distance
    };
    distance(one.chunk, other.chunk) <= u64::from(reach)
}

/// The places of what was sighted, in the order of their regions and, within a
/// region, of their chunks: so that nothing that is made of them depends on the order
/// in which they were given.
fn places(enter: ChunkPos, home: RegionId, regions: &[Sighted<'_>]) -> Vec<Place> {
    let mut fresh: BTreeMap<RegionId, bool> = BTreeMap::new();
    let mut crowds: BTreeMap<(RegionId, ChunkPos), u64> = BTreeMap::new();
    for sighted in regions {
        // A region that is given twice is fresh only if every word of it is: what is
        // not known for sure holds back.
        *fresh.entry(sighted.region).or_insert(true) &= sighted.fresh;
        for &(chunk, players) in sighted.crowds {
            // A sighting leaves out a chunk without players, and so does this.
            if players > 0 {
                let there = crowds.entry((sighted.region, chunk)).or_insert(0);
                *there = there.saturating_add(u64::from(players));
            }
        }
    }
    // The home region's one place more, whatever its sighting says and whether or not
    // it has one.
    crowds.entry((home, enter)).or_insert(0);
    crowds
        .into_iter()
        .map(|((region, chunk), players)| {
            let fresh = fresh.get(&region).copied().unwrap_or(false);
            Place {
                region,
                chunk,
                players,
                fresh,
                known: fresh || (region, chunk) == (home, enter),
            }
        })
        .collect()
}

/// The connected sets of those places that `among` holds of: for each such place the
/// lowest place of its set, and nothing for any other place.
///
/// Places are compared pair by pair, each with those that are in no set yet. That is
/// half a million comparisons at most for a thousand places, and section 2.5 of the
/// record says when a grid has to take the place of it.
fn clusters(
    policy: &Policy,
    places: &[Place],
    among: impl Fn(&Place) -> bool,
) -> Vec<Option<usize>> {
    let mut cluster = vec![None; places.len()];
    // The places that are in no set yet, ascending.
    let mut left: Vec<usize> = (0..places.len())
        .filter(|&place| among(&places[place]))
        .collect();
    let mut reached = Vec::new();
    while let Some(&first) = left.first() {
        cluster[first] = Some(first);
        reached.push(first);
        while let Some(one) = reached.pop() {
            left.retain(|&other| {
                if cluster[other].is_some() {
                    return false;
                }
                if !linked(policy, &places[one], &places[other]) {
                    return true;
                }
                cluster[other] = Some(first);
                reached.push(other);
                false
            });
        }
    }
    cluster
}

/// The merges and splits that are wanted: the splits in ascending order of their
/// regions, then the merges in ascending order of their gaps, then of the lower and
/// then of the higher of their two regions. See section 4 of the record.
///
/// `enter` is the chunk players enter the world in, `home` the home region, and
/// `regions` every region that has a sighting. Players of one region hold together
/// up to the split distance, players of two regions up to the merge distance, and
/// `enter` counts as a place where the home region has a player, whether or not it is
/// among `regions`. A region whose players are in several clusters is to be split,
/// and two regions with players within the merge distance of each other are to be
/// merged, unless the players in question are about to be split off. Nothing is
/// wanted of a region whose sighting is not fresh, nor of one that only such a
/// region's players hold together.
///
/// The answer does not depend on the order of `regions` or of the crowds of any of
/// them. A region that is given twice is one region with the players of both, and
/// fresh only if both are; a chunk that is given twice has the players of both; a
/// chunk with no players is not a place. Nothing is asked of the policy: distances
/// that were not [`Policy::checked`] are gone by as they are.
pub fn decide(
    policy: &Policy,
    enter: ChunkPos,
    home: RegionId,
    regions: &[Sighted<'_>],
) -> Vec<Wanted> {
    let places = places(enter, home, regions);
    // The clusters by all that was heard, and those by what is known.
    let heard = clusters(policy, &places, |_| true);
    let known = clusters(policy, &places, |place| place.known);

    let mut wanted = Vec::new();
    // How many players each region has by its sighting, and the places that count
    // for a merge.
    let mut players: BTreeMap<RegionId, u64> = BTreeMap::new();
    let mut counting: Vec<usize> = Vec::new();
    let mut first = 0;
    for of_region in places.chunk_by(|one, other| one.region == other.region) {
        let region = of_region[0].region;
        let its = first..first + of_region.len();
        first = its.end;
        let all = of_region
            .iter()
            .fold(0_u64, |sum, place| sum.saturating_add(place.players));
        players.insert(region, all);
        if !of_region[0].fresh {
            continue;
        }
        // Its groups: its places by the cluster they are in, each group ascending and
        // the groups in the order of their lowest chunks.
        let mut groups: BTreeMap<Option<usize>, Vec<usize>> = BTreeMap::new();
        for place in its.clone() {
            groups.entry(heard[place]).or_default().push(place);
        }
        let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
        groups.sort_by_key(|group| group[0]);
        if groups.len() > 1 {
            // Surely apart. In the home region the group with the chunk players enter
            // in stays, and in any other the one with the most players; of several
            // such the first, which has the lowest chunk.
            let weight = |group: &[usize]| {
                let enters =
                    region == home && group.iter().any(|&place| places[place].chunk == enter);
                let players = group.iter().fold(0_u64, |sum, &place| {
                    sum.saturating_add(places[place].players)
                });
                (enters, players)
            };
            let mut stays = 0;
            for (at, group) in groups.iter().enumerate() {
                if weight(group) > weight(&groups[stays]) {
                    stays = at;
                }
            }
            // Who stays takes in whoever comes near, and nobody is merged with
            // players that are about to go.
            counting.extend(groups.remove(stays));
            let chunks = |group: &Vec<usize>| -> Vec<ChunkPos> {
                group.iter().map(|&place| places[place].chunk).collect()
            };
            wanted.push(Wanted::Split {
                region,
                groups: groups.iter().map(chunks).collect(),
                why: Why::Apart,
            });
        } else if its.clone().all(|place| known[place] == known[its.start]) {
            // Surely whole.
            counting.extend(its);
        }
        // Else it is neither: only players that may not be there any more hold it
        // together, and nothing is wanted of it.
    }

    let mut gaps: BTreeMap<(RegionId, RegionId), u32> = BTreeMap::new();
    for (at, &one) in counting.iter().enumerate() {
        for &other in &counting[at + 1..] {
            let (one, other) = (&places[one], &places[other]);
            if one.region == other.region {
                continue;
            }
            // A distance that does not fit 32 bits is more than any merge distance.
            let Ok(apart) = u32::try_from(distance(one.chunk, other.chunk)) else {
                continue;
            };
            if apart > policy.merge_distance {
                continue;
            }
            let pair = (one.region.min(other.region), one.region.max(other.region));
            let gap = gaps.entry(pair).or_insert(apart);
            *gap = (*gap).min(apart);
        }
    }
    let mut merges: Vec<(u32, RegionId, RegionId)> = gaps
        .into_iter()
        .map(|((lower, higher), gap)| (gap, lower, higher))
        .collect();
    merges.sort_unstable();
    let players = |region: RegionId| players.get(&region).copied().unwrap_or(0);
    for (gap, lower, higher) in merges {
        // The home region survives; else the one with more players; else the lower.
        let higher_survives = higher == home || (lower != home && players(higher) > players(lower));
        let (survivor, absorbed) = if higher_survives {
            (higher, lower)
        } else {
            (lower, higher)
        };
        wanted.push(Wanted::Merge {
            survivor,
            absorbed,
            gap,
            why: Why::Near,
        });
    }
    wanted
}

/// The chunks a split names for `groups`: every chunk at most the margin away from a
/// chunk of one of them, ascending, each once. See section 4.3 of the record.
///
/// A chunk whose coordinate would lie beyond what a coordinate can be does not exist
/// and is left out. The coordinates are neither clamped, which would name a chunk
/// twice, nor wrapped, which would name chunks at the other end of the world.
pub fn named(policy: &Policy, groups: &[&[ChunkPos]]) -> Vec<ChunkPos> {
    let margin = i64::from(policy.margin());
    let around = |coordinate: i32| {
        (-margin..=margin).filter_map(move |by| i32::try_from(i64::from(coordinate) + by).ok())
    };
    let mut named = BTreeSet::new();
    for chunk in groups.iter().flat_map(|group| group.iter()) {
        for x in around(chunk.x) {
            for z in around(chunk.z) {
                named.insert(ChunkPos::new(x, z));
            }
        }
    }
    named.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(merge_distance: u32, split_distance: u32) -> Policy {
        Policy {
            merge_distance,
            split_distance,
            rest: Duration::from_secs(10),
        }
    }

    #[test]
    fn the_distances_follow_from_the_view_distance_and_the_rest_is_ten_seconds() {
        // What the edges grant unless told otherwise.
        let usual = Policy::for_view_distance(8);
        assert_eq!(usual, policy(22, 30));
        assert_eq!(usual.rest, Duration::from_secs(10));
        assert_eq!(usual.margin(), 3);
        // The least and the most an edge can be told to grant.
        assert_eq!(Policy::for_view_distance(2), policy(10, 18));
        assert_eq!(Policy::for_view_distance(32), policy(70, 78));
        // None at all is no view distance of an edge, and is reckoned with all the
        // same.
        assert_eq!(Policy::for_view_distance(0), policy(6, 14));
        for view_distance in 0..=32 {
            let policy = Policy::for_view_distance(view_distance);
            assert_eq!(policy.checked(), Ok(policy), "{view_distance}");
            assert_eq!(policy.margin(), 3, "{view_distance}");
        }
    }

    #[test]
    fn a_view_distance_whose_distances_do_not_fit_gives_the_largest_there_are() {
        // The largest that still leaves room for both distances.
        let largest = Policy::for_view_distance(u32::MAX / 2 - 7);
        assert_eq!(largest, policy(u32::MAX - 9, u32::MAX - 1));
        assert_eq!(largest.checked(), Ok(largest));
        // One more, and the split distance is no longer eight more than the other.
        let beyond = Policy::for_view_distance(u32::MAX / 2 - 6);
        assert_eq!(beyond, policy(u32::MAX - 7, u32::MAX));
        // And neither is what it would be. Such distances fit each other no more.
        let all = Policy::for_view_distance(u32::MAX);
        assert_eq!(all, policy(u32::MAX, u32::MAX));
        assert!(all.checked().is_err());
    }

    #[test]
    fn distances_are_accepted_when_the_split_distance_is_two_more_than_a_merge_distance_of_one_or_more()
     {
        // The smallest there are, and those the tests of the rules go by.
        for (merge_distance, split_distance) in [(1, 3), (2, 5), (1, 100), (22, 24)] {
            let policy = policy(merge_distance, split_distance);
            assert_eq!(policy.checked(), Ok(policy));
        }
        // Up to the largest that can be said.
        let largest = policy(u32::MAX - 2, u32::MAX);
        assert_eq!(largest.checked(), Ok(largest));
        // What is accepted comes back as it was, the rest with it, whatever that is.
        for rest in [Duration::ZERO, Duration::from_millis(1), Duration::MAX] {
            let policy = Policy {
                rest,
                ..policy(2, 5)
            };
            assert_eq!(policy.checked(), Ok(policy));
        }
    }

    #[test]
    fn a_merge_distance_of_nothing_is_refused() {
        for split_distance in [0, 2, 3, 30, u32::MAX] {
            assert_eq!(
                policy(0, split_distance).checked(),
                Err("the merge distance has to be 1 at least".to_owned())
            );
        }
    }

    #[test]
    fn a_split_distance_less_than_two_more_than_the_merge_distance_is_refused() {
        let refused = [
            (1, 0),
            (1, 1),
            (1, 2),
            (5, 4),
            (5, 5),
            (5, 6),
            (22, 23),
            (u32::MAX - 1, u32::MAX),
            (u32::MAX, u32::MAX),
            (u32::MAX, 3),
        ];
        for (merge_distance, split_distance) in refused {
            let refusal = policy(merge_distance, split_distance).checked();
            let why = format!(
                "the split distance has to be at least 2 more than the merge distance, \
                 and {split_distance} is not 2 more than {merge_distance}"
            );
            assert_eq!(refusal, Err(why));
        }
    }

    #[test]
    fn the_margin_is_three_or_as_much_as_the_split_distance_leaves_room_for() {
        let margins = [(3, 1), (4, 1), (5, 2), (6, 2), (7, 3), (8, 3), (30, 3)];
        for (split_distance, margin) in margins {
            assert_eq!(
                policy(1, split_distance).margin(),
                margin,
                "{split_distance}"
            );
        }
        assert_eq!(policy(1, u32::MAX).margin(), 3);
    }

    #[test]
    fn whatever_distances_are_accepted_have_a_margin_and_twice_of_it_is_less_than_the_split_distance()
     {
        for merge_distance in 0..=12 {
            for split_distance in 0..=40 {
                let Ok(policy) = policy(merge_distance, split_distance).checked() else {
                    continue;
                };
                let margin = policy.margin();
                assert!(margin >= 1, "{policy:?}");
                assert!(2 * margin < policy.split_distance, "{policy:?}");
                assert!(policy.split_distance >= 3, "{policy:?}");
            }
        }
    }

    #[test]
    fn the_margin_of_distances_that_were_not_checked_is_nothing_and_does_not_overflow() {
        assert_eq!(policy(0, 0).margin(), 0);
        assert_eq!(policy(0, 1).margin(), 0);
        assert_eq!(policy(9, 2).margin(), 0);
    }

    // What is wanted. Unless a test says otherwise the distances are 2 and 5, so the
    // margin is 2, region 0 is the home region and players enter at the origin.

    const HOME: RegionId = RegionId(0);
    const A: RegionId = RegionId(1);
    const B: RegionId = RegionId(2);
    const C: RegionId = RegionId(3);
    const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

    fn small() -> Policy {
        policy(2, 5)
    }

    /// What a region was last heard to have.
    #[derive(Debug, Clone)]
    struct Said {
        region: RegionId,
        fresh: bool,
        crowds: Crowds,
    }

    fn said(region: RegionId, fresh: bool, crowds: &[(i32, i32, u32)]) -> Said {
        Said {
            region,
            fresh,
            crowds: crowds
                .iter()
                .map(|&(x, z, players)| (ChunkPos::new(x, z), players))
                .collect(),
        }
    }

    /// A region with a fresh sighting of `crowds`: x, z and how many players.
    fn fresh(region: RegionId, crowds: &[(i32, i32, u32)]) -> Said {
        said(region, true, crowds)
    }

    /// A region whose sighting of `crowds` is not fresh.
    fn stale(region: RegionId, crowds: &[(i32, i32, u32)]) -> Said {
        said(region, false, crowds)
    }

    fn sighted(said: &[Said]) -> Vec<Sighted<'_>> {
        said.iter()
            .map(|said| Sighted {
                region: said.region,
                fresh: said.fresh,
                crowds: &said.crowds,
            })
            .collect()
    }

    fn wanted_by(policy: Policy, said: &[Said]) -> Vec<Wanted> {
        decide(&policy, ORIGIN, HOME, &sighted(said))
    }

    fn wanted(said: &[Said]) -> Vec<Wanted> {
        wanted_by(small(), said)
    }

    fn nothing() -> Vec<Wanted> {
        Vec::new()
    }

    fn merge(survivor: RegionId, absorbed: RegionId, gap: u32) -> Wanted {
        Wanted::Merge {
            survivor,
            absorbed,
            gap,
            why: Why::Near,
        }
    }

    fn chunks(chunks: &[(i32, i32)]) -> Vec<ChunkPos> {
        chunks.iter().map(|&(x, z)| ChunkPos::new(x, z)).collect()
    }

    fn split(region: RegionId, groups: &[&[(i32, i32)]]) -> Wanted {
        Wanted::Split {
            region,
            groups: groups.iter().map(|group| chunks(group)).collect(),
            why: Why::Apart,
        }
    }

    /// The chunks named for groups as `decide` gives them.
    fn named_of(policy: &Policy, groups: &[Vec<ChunkPos>]) -> Vec<ChunkPos> {
        let groups: Vec<&[ChunkPos]> = groups.iter().map(Vec::as_slice).collect();
        named(policy, &groups)
    }

    /// Every chunk at most `margin` from a chunk, ascending.
    fn square(x: i32, z: i32, margin: i32) -> Vec<ChunkPos> {
        (x - margin..=x + margin)
            .flat_map(|x| (z - margin..=z + margin).map(move |z| ChunkPos::new(x, z)))
            .collect()
    }

    #[test]
    fn the_distance_is_along_the_longer_axis_and_fits_the_whole_world() {
        let at = ChunkPos::new;
        assert_eq!(distance(at(3, -4), at(3, -4)), 0);
        assert_eq!(distance(at(0, 0), at(2, -7)), 7);
        assert_eq!(distance(at(-5, 1), at(4, 2)), 9);
        assert_eq!(distance(at(4, 2), at(-5, 1)), 9);
        let (first, last) = (i32::MIN, i32::MAX);
        assert_eq!(distance(at(first, 0), at(last, 0)), u64::from(u32::MAX));
        assert_eq!(distance(at(last, last), at(0, first)), u64::from(u32::MAX));
        assert_eq!(distance(at(first, first), at(first, first + 1)), 1);
    }

    // D1.
    #[test]
    fn two_regions_are_merged_when_their_players_are_within_the_merge_distance_along_the_longer_axis()
     {
        let apart = |x: i32, z: i32| {
            wanted(&[
                fresh(HOME, &[]),
                fresh(A, &[(100, 100, 1)]),
                fresh(B, &[(100 + x, 100 + z, 1)]),
            ])
        };
        assert_eq!(apart(2, 0), [merge(A, B, 2)]);
        assert_eq!(apart(3, 0), nothing());
        assert_eq!(apart(2, 2), [merge(A, B, 2)]);
        assert_eq!(apart(0, 3), nothing());
        // The other way along either axis, and nearer than that.
        assert_eq!(apart(-2, 1), [merge(A, B, 2)]);
        assert_eq!(apart(-3, 0), nothing());
        assert_eq!(apart(1, -3), nothing());
        assert_eq!(apart(-1, 1), [merge(A, B, 1)]);
        // In one chunk, as when a player is in two sightings for a report.
        assert_eq!(apart(0, 0), [merge(A, B, 0)]);
    }

    #[test]
    fn the_gap_of_a_merge_is_between_the_nearest_players_of_the_two_regions() {
        // Not between the first of them that are near enough.
        let near = wanted(&[
            fresh(A, &[(100, 0, 1), (104, 0, 1)]),
            fresh(B, &[(102, 0, 1), (105, 0, 1)]),
        ]);
        assert_eq!(near, [merge(A, B, 1)]);
        let near = wanted(&[
            fresh(A, &[(100, 0, 1), (104, 0, 1), (103, 3, 1)]),
            fresh(B, &[(106, 0, 1), (104, 3, 1), (108, 5, 1)]),
        ]);
        assert_eq!(near, [merge(A, B, 1)]);
    }

    #[test]
    fn two_regions_are_not_merged_for_being_in_one_cluster_but_for_players_of_their_own_that_are_near()
     {
        // `C` is within the merge distance of both, and they are not of each other.
        let row = wanted(&[
            fresh(A, &[(100, 0, 1)]),
            fresh(B, &[(104, 0, 1)]),
            fresh(C, &[(102, 0, 1)]),
        ]);
        assert_eq!(row, [merge(A, C, 2), merge(B, C, 2)]);
    }

    // D2.
    #[test]
    fn the_home_region_survives_a_merge_then_the_region_with_more_players_then_the_lower() {
        // The home region with one player against a region with five.
        let home = wanted(&[fresh(HOME, &[(1, 0, 1)]), fresh(A, &[(3, 0, 5)])]);
        assert_eq!(home, [merge(HOME, A, 2)]);
        // Also when it is the higher of the two, and has nobody.
        let regions = [fresh(A, &[(2, 0, 5)]), fresh(C, &[])];
        let higher = decide(&small(), ORIGIN, C, &sighted(&regions));
        assert_eq!(higher, [merge(C, A, 2)]);

        // Of two others the one with more players, in however many chunks.
        let more = |a: &[(i32, i32, u32)], b: &[(i32, i32, u32)]| {
            wanted(&[fresh(HOME, &[]), fresh(A, a), fresh(B, b)])
        };
        assert_eq!(more(&[(100, 0, 2)], &[(102, 0, 3)]), [merge(B, A, 2)]);
        assert_eq!(more(&[(100, 0, 3)], &[(102, 0, 2)]), [merge(A, B, 2)]);
        assert_eq!(
            more(&[(100, 0, 2)], &[(102, 0, 1), (103, 0, 1), (104, 4, 1)]),
            [merge(B, A, 2)]
        );
        // Of two with as many the lower.
        assert_eq!(
            more(&[(100, 0, 3)], &[(102, 0, 1), (103, 0, 2)]),
            [merge(A, B, 2)]
        );
    }

    #[test]
    fn the_players_of_a_region_that_are_to_go_count_for_which_region_survives() {
        // Three of `A` stay and four go, in two groups. `B` has five, near those who
        // stay: the merge is of all of `A` as it is, and `A` has more.
        let both = wanted(&[
            fresh(A, &[(100, 0, 3), (110, 0, 2), (120, 0, 2)]),
            fresh(B, &[(98, 0, 5)]),
        ]);
        assert_eq!(
            both,
            [split(A, &[&[(110, 0)], &[(120, 0)]]), merge(A, B, 2)]
        );
    }

    // D3.
    #[test]
    fn a_region_near_the_chunk_players_enter_in_is_merged_into_a_home_region_that_is_fresh() {
        // Without a sighting the home region is not fresh.
        assert_eq!(wanted(&[fresh(A, &[(2, 0, 1)])]), nothing());
        assert_eq!(
            wanted(&[stale(HOME, &[]), fresh(A, &[(2, 0, 1)])]),
            nothing()
        );
        // Fresh and empty: nobody may be there, and the region is merged into it.
        assert_eq!(
            wanted(&[fresh(HOME, &[]), fresh(A, &[(2, 0, 1)])]),
            [merge(HOME, A, 2)]
        );
        assert_eq!(
            wanted(&[fresh(HOME, &[]), fresh(A, &[(-1, 2, 4)])]),
            [merge(HOME, A, 2)]
        );
        assert_eq!(
            wanted(&[fresh(HOME, &[]), fresh(A, &[(3, 0, 1)])]),
            nothing()
        );
        // It is where players enter that counts, wherever that is.
        let regions = [fresh(HOME, &[]), fresh(A, &[(2, 0, 1)])];
        let enter = ChunkPos::new(4, -2);
        assert_eq!(
            decide(&small(), enter, HOME, &sighted(&regions)),
            [merge(HOME, A, 2)]
        );
        let enter = ChunkPos::new(5, 0);
        assert_eq!(decide(&small(), enter, HOME, &sighted(&regions)), nothing());
    }

    // D4.
    #[test]
    fn a_region_is_split_when_its_players_are_further_apart_than_the_split_distance() {
        let apart =
            |x: i32, z: i32| wanted(&[fresh(HOME, &[]), fresh(A, &[(100, 0, 1), (100 + x, z, 1)])]);
        assert_eq!(apart(5, 0), nothing());
        assert_eq!(apart(6, 0), [split(A, &[&[(106, 0)]])]);
        assert_eq!(apart(5, 5), nothing());
        assert_eq!(apart(0, 6), [split(A, &[&[(100, 6)]])]);
        // Players in between hold the others together.
        let row = wanted(&[fresh(A, &[(100, 0, 1), (105, 0, 1), (110, 0, 1)])]);
        assert_eq!(row, nothing());
    }

    // D5.
    #[test]
    fn the_group_with_more_players_stays_and_of_two_with_as_many_the_one_with_the_lowest_chunk() {
        let more = wanted(&[fresh(A, &[(100, 0, 1), (106, 0, 2)])]);
        assert_eq!(more, [split(A, &[&[(100, 0)]])]);
        // Players are counted, not chunks.
        let counted = wanted(&[fresh(A, &[(100, 0, 1), (101, 0, 1), (107, 0, 3)])]);
        assert_eq!(counted, [split(A, &[&[(100, 0), (101, 0)]])]);

        let as_many = wanted(&[fresh(A, &[(100, 0, 1), (101, 3, 1), (107, 0, 2)])]);
        assert_eq!(as_many, [split(A, &[&[(107, 0)]])]);
        // Chunks are in order of x and then of z, and it is the lowest chunk of the
        // group that counts.
        let along_z = wanted(&[fresh(A, &[(100, 50, 1), (100, 40, 1)])]);
        assert_eq!(along_z, [split(A, &[&[(100, 50)]])]);
        let lowest = wanted(&[fresh(A, &[(107, -4, 2), (104, 9, 1), (105, 5, 1)])]);
        assert_eq!(lowest, [split(A, &[&[(107, -4)]])]);
    }

    // D5.
    #[test]
    fn a_group_that_goes_is_given_as_its_chunks_ascending_and_named_as_the_squares_around_them() {
        let regions = [fresh(
            A,
            &[(107, -1, 1), (100, 0, 9), (106, 1, 1), (106, 0, 1)],
        )];
        let wanted = wanted(&regions);
        assert_eq!(wanted, [split(A, &[&[(106, 0), (106, 1), (107, -1)]])]);
        let Wanted::Split { groups, .. } = &wanted[0] else {
            panic!("a split was wanted");
        };
        // Each chunk once, although the three squares lie over each other.
        let around = named_of(&small(), groups);
        let squares: BTreeSet<ChunkPos> = [(106, 0), (106, 1), (107, -1)]
            .into_iter()
            .flat_map(|(x, z)| square(x, z, 2))
            .collect();
        assert_eq!(around, squares.into_iter().collect::<Vec<_>>());
        // Six chunks of the column furthest west, seven of each of the four in the
        // middle and five of the one furthest east.
        assert_eq!(around.len(), 6 + 4 * 7 + 5);
        assert_eq!(around.first(), Some(&ChunkPos::new(104, -2)));
        assert_eq!(around.last(), Some(&ChunkPos::new(109, 1)));

        // Around one chunk it is the square, in order of x and then of z.
        let one = named(&small(), &[&[ChunkPos::new(106, 0)]]);
        assert_eq!(one, square(106, 0, 2));
        assert_eq!(one.len(), 25);
        let mut ascending = one.clone();
        ascending.sort();
        assert_eq!(one, ascending);
        // And with the distances that are usual, 49 chunks.
        let usual = Policy::for_view_distance(8);
        let one = named(&usual, &[&[ChunkPos::new(-7, 7)]]);
        assert_eq!(one, square(-7, 7, 3));
        assert_eq!(one.len(), 49);
    }

    #[test]
    fn the_chunks_named_are_in_order_and_each_once_however_the_groups_are_given() {
        let at = ChunkPos::new;
        // Not in order, a chunk twice, a group twice, a group with nothing.
        let given = named(
            &small(),
            &[
                &[at(11, 0), at(10, 0), at(11, 0)],
                &[],
                &[at(10, 0)],
                &[at(30, -1)],
            ],
        );
        let mut expected: Vec<ChunkPos> = (8..=13)
            .flat_map(|x| (-2..=2).map(move |z| at(x, z)))
            .collect();
        expected.extend(square(30, -1, 2));
        assert_eq!(given, expected);
        // No groups, no chunks.
        assert_eq!(named(&small(), &[]), Vec::new());
        assert_eq!(named(&small(), &[&[]]), Vec::new());
    }

    #[test]
    fn the_chunks_named_by_distances_that_leave_no_margin_are_those_of_the_groups() {
        let at = ChunkPos::new;
        // A split distance of 3 leaves a margin of 1.
        assert_eq!(named(&policy(1, 3), &[&[at(5, 5)]]), square(5, 5, 1));
        // Distances that were not checked leave none.
        let none = policy(0, 0);
        assert_eq!(
            named(&none, &[&[at(5, 5), at(4, 9)], &[at(5, 5)]]),
            [at(4, 9), at(5, 5)]
        );
    }

    // D6.
    #[test]
    fn in_the_home_region_the_group_where_players_enter_stays_whoever_is_there() {
        // One player near the origin and five far from it.
        let outnumbered = wanted(&[fresh(HOME, &[(1, 0, 1), (10, 0, 5)])]);
        assert_eq!(outnumbered, [split(HOME, &[&[(10, 0)]])]);
        // Nobody near the origin at all: its only players are 6 from it.
        let alone = wanted(&[fresh(HOME, &[(6, 0, 3)])]);
        assert_eq!(alone, [split(HOME, &[&[(6, 0)]])]);
        let alone = wanted(&[fresh(HOME, &[(-6, 6, 1), (-7, 6, 1)])]);
        assert_eq!(alone, [split(HOME, &[&[(-7, 6), (-6, 6)]])]);
        // At 5 from it they are held.
        assert_eq!(wanted(&[fresh(HOME, &[(5, -5, 3)])]), nothing());
        // The chunk players enter in is not given with a group, although it is the
        // lowest chunk there is here.
        let regions = [fresh(HOME, &[(0, 0, 1), (9, 9, 1), (20, 20, 7)])];
        let enter = ChunkPos::new(9, 9);
        assert_eq!(
            decide(&small(), enter, HOME, &sighted(&regions)),
            [split(HOME, &[&[(0, 0)], &[(20, 20)]])]
        );
        // A home region that is not fresh is not split.
        assert_eq!(wanted(&[stale(HOME, &[(6, 0, 3)])]), nothing());
    }

    // D7.
    #[test]
    fn every_group_that_does_not_stay_goes_each_as_its_own_chunks_in_the_order_of_their_lowest() {
        let regions = [fresh(
            A,
            &[
                (120, 0, 1),
                (100, 1, 1),
                (110, 0, 3),
                (100, 0, 1),
                (121, 1, 1),
            ],
        )];
        let wanted = wanted(&regions);
        assert_eq!(
            wanted,
            [split(A, &[&[(100, 0), (100, 1)], &[(120, 0), (121, 1)]])]
        );
        let Wanted::Split { groups, .. } = &wanted[0] else {
            panic!("a split was wanted");
        };

        let both = named_of(&small(), groups);
        let first = named_of(&small(), &groups[..1]);
        let second = named_of(&small(), &groups[1..]);
        // What is named of both is what is named of each, each chunk once.
        let mut each: Vec<ChunkPos> = first.iter().chain(&second).copied().collect();
        each.sort();
        assert_eq!(both, each);
        assert_eq!(first.len(), 5 * 6);
        assert_eq!(second.len(), 2 * 25 - 16);
        // And of one alone there is no chunk that only the other has.
        assert!(first.iter().all(|chunk| !second.contains(chunk)));
        assert!(first.iter().all(|chunk| chunk.x <= 102));
        assert!(second.iter().all(|chunk| chunk.x >= 118));
        // Nothing is named around the group that stays.
        assert!(both.iter().all(|chunk| chunk.x <= 102 || chunk.x >= 118));
    }

    // D7.
    #[test]
    fn the_groups_that_go_are_in_the_order_of_their_chunks_whatever_else_is_in_their_clusters() {
        // The further group is in a cluster with a region that comes before this one.
        let regions = [
            fresh(A, &[(122, 0, 1)]),
            fresh(B, &[(120, 0, 1), (100, 0, 5), (110, 0, 1)]),
        ];
        assert_eq!(wanted(&regions), [split(B, &[&[(110, 0)], &[(120, 0)]])]);
    }

    // D8, K2 (a).
    #[test]
    fn a_region_is_split_and_not_merged_with_the_region_near_its_group_that_goes() {
        let regions = [
            fresh(HOME, &[]),
            fresh(A, &[(100, 0, 3), (110, 0, 1)]),
            fresh(C, &[(112, 0, 1)]),
        ];
        assert_eq!(wanted(&regions), [split(A, &[&[(110, 0)]])]);
        // However many `C` has there.
        let regions = [
            fresh(A, &[(100, 0, 3), (110, 0, 1)]),
            fresh(C, &[(112, 0, 9), (111, 1, 9)]),
        ];
        assert_eq!(wanted(&regions), [split(A, &[&[(110, 0)]])]);
    }

    // D8, K2 (b).
    #[test]
    fn a_region_whose_groups_another_region_joins_is_whole_and_merged_with_that_one() {
        let regions = [
            fresh(HOME, &[]),
            fresh(A, &[(100, 0, 3), (109, 0, 1)]),
            fresh(C, &[(102, 0, 1), (107, 0, 1)]),
        ];
        assert_eq!(wanted(&regions), [merge(A, C, 2)]);
    }

    // D8.
    #[test]
    fn a_region_that_is_apart_is_split_and_merged_with_the_region_near_its_group_that_stays() {
        let regions = [
            fresh(HOME, &[]),
            fresh(A, &[(100, 0, 3), (110, 0, 1)]),
            fresh(B, &[(98, 0, 1)]),
        ];
        assert_eq!(wanted(&regions), [split(A, &[&[(110, 0)]]), merge(A, B, 2)]);
        // With a region near each group, only the one near those who stay.
        let regions = [
            fresh(A, &[(100, 0, 3), (110, 0, 1)]),
            fresh(B, &[(98, 0, 1)]),
            fresh(C, &[(112, 0, 1)]),
        ];
        assert_eq!(wanted(&regions), [split(A, &[&[(110, 0)]]), merge(A, B, 2)]);
        // Two regions that are both apart are merged by their groups that stay.
        let regions = [
            fresh(A, &[(100, 0, 3), (110, 0, 1)]),
            fresh(B, &[(98, 0, 2), (88, 0, 1)]),
        ];
        assert_eq!(
            wanted(&regions),
            [
                split(A, &[&[(110, 0)]]),
                split(B, &[&[(88, 0)]]),
                merge(A, B, 2)
            ]
        );
        // And not by a group that goes and one that stays.
        let regions = [
            fresh(A, &[(100, 0, 3), (110, 0, 1)]),
            fresh(B, &[(112, 0, 2), (122, 0, 1)]),
        ];
        assert_eq!(
            wanted(&regions),
            [split(A, &[&[(110, 0)]]), split(B, &[&[(122, 0)]])]
        );
    }

    // D9, with two players of `C`: one cannot be within 2 of each of two groups that
    // are more than 5 apart.
    #[test]
    fn nothing_is_wanted_of_a_region_whose_groups_only_a_sighting_that_is_not_fresh_joins() {
        let joined = |c: Said| {
            wanted(&[
                fresh(HOME, &[]),
                fresh(A, &[(100, 0, 1), (109, 0, 1)]),
                fresh(B, &[(98, 0, 1)]),
                c,
            ])
        };
        // Neither a split of `A` nor a merge of `A` with `B`, which is fresh and near.
        let between = [(102, 0, 1), (107, 0, 1)];
        assert_eq!(joined(stale(C, &between)), nothing());
        // Were `C` fresh, `A` would be whole.
        assert_eq!(joined(fresh(C, &between)), [merge(A, B, 2), merge(A, C, 2)]);
        // And were its players elsewhere, `A` would be apart.
        assert_eq!(
            joined(stale(C, &[(102, 9, 1), (107, 9, 1)])),
            [split(A, &[&[(109, 0)]]), merge(A, B, 2)]
        );
        assert_eq!(
            joined(stale(C, &[])),
            [split(A, &[&[(109, 0)]]), merge(A, B, 2)]
        );

        // The same at the origin, where the home region's place is as well.
        let at_origin = |c: Said| wanted(&[fresh(HOME, &[]), fresh(A, &[(0, 0, 1), (9, 0, 1)]), c]);
        assert_eq!(at_origin(stale(C, &[(2, 0, 1), (7, 0, 1)])), nothing());
        assert_eq!(
            at_origin(stale(C, &[])),
            [split(A, &[&[(9, 0)]]), merge(HOME, A, 0)]
        );
    }

    #[test]
    fn one_player_of_a_sighting_that_is_not_fresh_joins_two_groups_where_the_distances_allow_it() {
        // With distances 3 and 5 one player can be within 3 of each of two groups
        // that are 6 apart.
        let joined = |c: Said| wanted_by(policy(3, 5), &[fresh(A, &[(100, 0, 1), (106, 0, 1)]), c]);
        assert_eq!(joined(stale(C, &[(103, 0, 1)])), nothing());
        assert_eq!(joined(fresh(C, &[(103, 0, 1)])), [merge(A, C, 3)]);
        assert_eq!(joined(stale(C, &[])), [split(A, &[&[(106, 0)]])]);
    }

    #[test]
    fn players_of_a_home_region_that_is_not_fresh_join_groups_only_as_those_of_any_such_region() {
        let usual = Policy::for_view_distance(8);
        // `A` has a player 5 from the origin and one 45 from it, who is 20 from a
        // player of the home region, who is 25 from the origin.
        let a = fresh(A, &[(5, 0, 1), (45, 0, 1)]);
        let known = wanted_by(usual, &[fresh(HOME, &[(25, 0, 1)]), a.clone()]);
        assert_eq!(known, [merge(HOME, A, 5)]);
        let unknown = wanted_by(usual, &[stale(HOME, &[(25, 0, 1)]), a.clone()]);
        assert_eq!(unknown, nothing());
        // Without that player `A` is apart: the one near the origin is in the group
        // of the chunk players enter in, and the other alone.
        let gone = wanted_by(usual, &[stale(HOME, &[]), a]);
        assert_eq!(gone, [split(A, &[&[(45, 0)]])]);
    }

    #[test]
    fn the_chunk_players_enter_in_holds_players_together_whatever_is_known_of_the_home_region() {
        let usual = Policy::for_view_distance(8);
        // Two players 40 apart are split anywhere else.
        let elsewhere = wanted_by(usual, &[fresh(A, &[(80, 0, 1), (120, 0, 1)])]);
        assert_eq!(elsewhere, [split(A, &[&[(120, 0)]])]);
        // Each 20 from the origin, they are both where the home region has a place.
        let around = fresh(A, &[(-20, 0, 1), (20, 0, 1)]);
        let fresh_home = wanted_by(usual, &[fresh(HOME, &[]), around.clone()]);
        assert_eq!(fresh_home, [merge(HOME, A, 20)]);
        // That place is known for sure whatever the home region's sighting is, so
        // the region is whole, and is merged with others, though not with the home
        // region while that is not fresh.
        for home in [
            None,
            Some(stale(HOME, &[])),
            Some(stale(HOME, &[(0, 0, 2)])),
        ] {
            let mut regions = vec![around.clone()];
            regions.extend(home);
            assert_eq!(wanted_by(usual, &regions), nothing());
            regions.push(fresh(B, &[(42, 0, 1)]));
            assert_eq!(wanted_by(usual, &regions), [merge(A, B, 22)]);
        }
    }

    // D10.
    #[test]
    fn a_region_that_is_not_fresh_is_in_no_merge_and_no_split_whatever_it_holds() {
        // Apart, and beside a fresh region.
        let regions = [
            fresh(HOME, &[]),
            stale(A, &[(100, 0, 3), (110, 0, 1)]),
            fresh(B, &[(98, 0, 1)]),
        ];
        assert_eq!(wanted(&regions), nothing());
        // In the very chunk of a fresh region, and at the origin.
        let regions = [
            fresh(HOME, &[(1, 1, 1)]),
            stale(A, &[(1, 1, 1), (0, 0, 4)]),
            fresh(B, &[(50, 0, 1)]),
            stale(C, &[(50, 0, 1), (60, 0, 1)]),
        ];
        assert_eq!(wanted(&regions), nothing());
        // Two that are not fresh beside each other.
        let regions = [stale(A, &[(100, 0, 1)]), stale(B, &[(101, 0, 1)])];
        assert_eq!(wanted(&regions), nothing());
        // The home region, with players far from where players enter.
        let regions = [stale(HOME, &[(40, 0, 1)]), fresh(A, &[(41, 0, 1)])];
        assert_eq!(wanted(&regions), nothing());
    }

    // D12.
    #[test]
    fn splits_come_first_by_region_and_then_merges_by_gap_by_the_lower_and_by_the_higher_region() {
        let region = RegionId;
        let regions = [
            fresh(region(12), &[(1000, 200, 1)]),
            fresh(region(8), &[(1002, 502, 1)]),
            fresh(region(5), &[(1000, 0, 1), (1010, 0, 1)]),
            fresh(region(10), &[(999, 300, 1)]),
            fresh(region(6), &[(1000, 401, 3)]),
            fresh(region(3), &[(1000, 300, 1)]),
            fresh(region(7), &[(1000, 500, 1)]),
            fresh(region(2), &[(1000, 100, 1), (1010, 100, 1)]),
            fresh(region(9), &[(1001, 300, 1)]),
            fresh(region(4), &[(1000, 400, 1)]),
            fresh(region(11), &[(1000, 200, 1)]),
        ];
        assert_eq!(
            wanted(&regions),
            [
                split(region(2), &[&[(1010, 100)]]),
                split(region(5), &[&[(1010, 0)]]),
                merge(region(11), region(12), 0),
                merge(region(3), region(9), 1),
                merge(region(3), region(10), 1),
                // By the lower of the two, whichever of them survives.
                merge(region(6), region(4), 1),
                merge(region(7), region(8), 2),
                merge(region(9), region(10), 2),
            ]
        );
    }

    // D13.
    #[test]
    fn players_at_the_ends_of_the_world_are_no_nearer_to_each_other_for_it() {
        let (first, last) = (i32::MIN, i32::MAX);
        // In 32 bits the two ends would be one chunk apart.
        for (a, b) in [
            ((first, 0), (last, 0)),
            ((0, first), (0, last)),
            ((first, first), (last, last)),
            ((first, last), (last, first)),
        ] {
            let two = wanted(&[fresh(A, &[(a.0, a.1, 1)]), fresh(B, &[(b.0, b.1, 1)])]);
            assert_eq!(two, nothing(), "{a:?} {b:?}");
            let one = wanted(&[fresh(A, &[(a.0, a.1, 1), (b.0, b.1, 1)])]);
            assert_eq!(one, [split(A, &[&[b]])], "{a:?} {b:?}");
        }
        // At either end things are as they are anywhere.
        let east = |x: i32, z: i32| {
            wanted(&[
                fresh(A, &[(last, last, 1)]),
                fresh(B, &[(last - x, last - z, 1)]),
            ])
        };
        assert_eq!(east(2, 1), [merge(A, B, 2)]);
        assert_eq!(east(0, 3), nothing());
        let west = |x: i32, z: i32| {
            wanted(&[
                fresh(A, &[(first, first, 1), (first + x, first + z, 1)]),
                fresh(B, &[(first + 1, first + 2, 1)]),
            ])
        };
        assert_eq!(west(5, 0), [merge(A, B, 2)]);
        assert_eq!(
            west(0, 6),
            [split(A, &[&[(first, first + 6)]]), merge(A, B, 2)]
        );
    }

    // D13.
    #[test]
    fn the_largest_distances_there_are_are_gone_by_without_overflow() {
        let (first, last) = (i32::MIN, i32::MAX);
        let largest = policy(u32::MAX - 2, u32::MAX);
        assert_eq!(largest.checked(), Ok(largest));
        // The world is as wide as the split distance: a region holds together from
        // one end to the other.
        let one = wanted_by(largest, &[fresh(A, &[(first, 0, 1), (last, last, 1)])]);
        assert_eq!(one, nothing());
        // Two regions are merged up to two chunks less than that.
        let two = |x: i32| {
            wanted_by(
                largest,
                &[
                    fresh(A, &[(first + x, first, 1)]),
                    fresh(B, &[(last, 0, 1)]),
                ],
            )
        };
        assert_eq!(two(0), nothing());
        assert_eq!(two(1), nothing());
        assert_eq!(two(2), [merge(A, B, u32::MAX - 2)]);
        assert_eq!(two(3), [merge(A, B, u32::MAX - 3)]);
    }

    // D13.
    #[test]
    fn the_chunks_named_at_the_end_of_the_world_leave_out_those_beyond_it() {
        let at = ChunkPos::new;
        let (first, last) = (i32::MIN, i32::MAX);
        let around = |x: i32, z: i32| named(&small(), &[&[at(x, z)]]);
        // At the last chunk along x: three columns of the five, and every other
        // chunk of the square once.
        let east: Vec<ChunkPos> = (last - 2..=last)
            .flat_map(|x| (-2..=2).map(move |z| at(x, z)))
            .collect();
        assert_eq!(around(last, 0), east);
        assert_eq!(east.len(), 15);
        // One short of it, four.
        assert_eq!(around(last - 1, 0).len(), 20);
        assert_eq!(around(last - 2, 0), square(last - 2, 0, 2));
        // At the first chunk along z.
        let north: Vec<ChunkPos> = (-2..=2)
            .flat_map(|x| (first..=first + 2).map(move |z| at(x, z)))
            .collect();
        assert_eq!(around(0, first), north);
        // In the corners.
        let corner: Vec<ChunkPos> = (last - 2..=last)
            .flat_map(|x| (first..=first + 2).map(move |z| at(x, z)))
            .collect();
        assert_eq!(around(last, first), corner);
        let corner: Vec<ChunkPos> = (first..=first + 2)
            .flat_map(|x| (last - 2..=last).map(move |z| at(x, z)))
            .collect();
        assert_eq!(around(first, last), corner);
        // Nothing is named at the other end of the world, and nothing twice.
        let both = named(&small(), &[&[at(last, last)], &[at(last, last - 1)]]);
        assert_eq!(both.len(), 3 * 4);
        assert!(
            both.iter()
                .all(|chunk| chunk.x >= last - 2 && chunk.z >= last - 3)
        );
    }

    // D14.
    #[test]
    fn a_region_without_players_is_in_nothing() {
        // Beside players, and with chunks that it says nobody is in.
        let regions = [
            fresh(HOME, &[]),
            fresh(A, &[]),
            fresh(B, &[(100, 0, 1)]),
            fresh(C, &[(100, 0, 0), (101, 0, 0), (120, 0, 0)]),
        ];
        assert_eq!(wanted(&regions), nothing());
        // Nor does a chunk without players hold others together.
        let regions = [
            fresh(A, &[(100, 0, 1), (105, 0, 0), (110, 0, 1)]),
            fresh(B, &[(112, 0, 0)]),
        ];
        assert_eq!(wanted(&regions), [split(A, &[&[(110, 0)]])]);
        // Nothing at all is nothing wanted.
        assert_eq!(wanted(&[]), nothing());
        assert_eq!(wanted(&[fresh(HOME, &[])]), nothing());
    }

    // D15, with two players of `C`, 5 from each other and each within 2 of one set.
    #[test]
    fn players_of_a_fresh_region_between_two_sets_make_them_one_group_and_a_merge() {
        let a = fresh(A, &[(100, 0, 1), (99, 1, 1), (109, 0, 1), (110, 1, 1)]);
        let with = wanted(&[
            fresh(HOME, &[]),
            a.clone(),
            fresh(C, &[(102, 0, 1), (107, 0, 1)]),
        ]);
        assert_eq!(with, [merge(A, C, 2)]);
        // Without `C` they are two groups.
        let without = wanted(&[fresh(HOME, &[]), a.clone()]);
        assert_eq!(without, [split(A, &[&[(109, 0), (110, 1)]])]);
        // With only one of the two players as well: `C` is then merged if it is
        // near those who stay.
        let stays = wanted(&[a.clone(), fresh(C, &[(102, 0, 1)])]);
        assert_eq!(stays, [split(A, &[&[(109, 0), (110, 1)]]), merge(A, C, 2)]);
        let goes = wanted(&[a, fresh(C, &[(107, 0, 1)])]);
        assert_eq!(goes, [split(A, &[&[(109, 0), (110, 1)]])]);
    }

    #[test]
    fn a_region_that_is_given_twice_is_one_region_and_fresh_only_if_both_are() {
        let once = wanted(&[fresh(A, &[(100, 0, 2), (106, 0, 3)])]);
        assert_eq!(once, [split(A, &[&[(100, 0)]])]);
        let twice = wanted(&[fresh(A, &[(106, 0, 3)]), fresh(A, &[(100, 0, 2)])]);
        assert_eq!(twice, once);
        // The players of both are counted.
        let twice = wanted(&[
            fresh(A, &[(106, 0, 3), (100, 0, 2)]),
            fresh(A, &[(100, 0, 2)]),
        ]);
        assert_eq!(twice, [split(A, &[&[(106, 0)]])]);
        // One of the two not fresh, whichever comes first: nothing is wanted of it.
        let parts = [fresh(A, &[(106, 0, 3)]), stale(A, &[(100, 0, 2)])];
        assert_eq!(wanted(&parts), nothing());
        let parts = [stale(A, &[(100, 0, 2)]), fresh(A, &[(106, 0, 3)])];
        assert_eq!(wanted(&parts), nothing());
    }

    #[test]
    fn a_chunk_that_is_given_twice_has_the_players_of_both() {
        let regions = [fresh(A, &[(100, 0, 1), (106, 0, 2), (100, 0, 2)])];
        assert_eq!(wanted(&regions), [split(A, &[&[(106, 0)]])]);
        // More players than fit 32 bits are counted all the same.
        let regions = [
            fresh(A, &[(100, 0, u32::MAX)]),
            fresh(B, &[(102, 0, u32::MAX), (102, 0, 1)]),
        ];
        assert_eq!(wanted(&regions), [merge(B, A, 2)]);
    }

    #[test]
    fn distances_that_were_not_checked_are_gone_by_as_they_are() {
        // No distance at all: only regions in one chunk are merged, and every chunk
        // of a region is a group of its own.
        let regions = [
            fresh(A, &[(100, 0, 1), (101, 0, 2)]),
            fresh(B, &[(101, 0, 1), (103, 0, 1)]),
        ];
        assert_eq!(
            wanted_by(policy(0, 0), &regions),
            [
                split(A, &[&[(100, 0)]]),
                split(B, &[&[(103, 0)]]),
                merge(A, B, 0)
            ]
        );
        // A merge distance beyond the split distance.
        assert_eq!(wanted_by(policy(5, 1), &regions), [merge(A, B, 0)]);
        let regions = [fresh(A, &[(100, 0, 2), (104, 0, 1)]), fresh(B, &[])];
        assert_eq!(
            wanted_by(policy(5, 1), &regions),
            [split(A, &[&[(104, 0)]])]
        );
    }

    // What follows is generated.

    /// Numbers from a seed (SplitMix64), the same on every machine.
    struct Dice(u64);

    impl Dice {
        fn roll(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut mixed = self.0;
            mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            mixed ^ (mixed >> 31)
        }

        /// A number below `bound`.
        fn below(&mut self, bound: u64) -> u64 {
            self.roll() % bound
        }

        /// A number below `bound`, as a step along an axis.
        fn step(&mut self, bound: u32) -> i32 {
            i32::try_from(self.below(u64::from(bound))).expect("a small bound")
        }

        /// True once in `times` times.
        fn once_in(&mut self, times: u64) -> bool {
            self.below(times) == 0
        }

        fn shuffle<T>(&mut self, things: &mut [T]) {
            for last in (1..things.len()).rev() {
                let other = self.below(last as u64 + 1) as usize;
                things.swap(last, other);
            }
        }
    }

    /// A world to decide about.
    #[derive(Debug, Clone)]
    struct Case {
        policy: Policy,
        enter: ChunkPos,
        home: RegionId,
        said: Vec<Said>,
    }

    impl Case {
        /// The world of a seed: up to six of seven regions, the home region not
        /// always among them, one in three not fresh, in a strip that is three split
        /// distances long and four chunks wide, so that some of their players are
        /// linked and some are not. One world in four lies in a corner of the world.
        fn of(seed: u64) -> Self {
            let mut dice = Dice(seed);
            let distances = [(2, 5), (1, 3), (3, 5), (2, 9), (4, 6), (5, 7), (6, 8)];
            let (merge_distance, split_distance) = distances[dice.below(7) as usize];
            let (long, wide) = (3 * split_distance, 4);
            let reach = i32::try_from(long).expect("a small distance");
            let (west, north) = match dice.below(8) {
                0 => (i32::MAX - reach, i32::MIN),
                1 => (i32::MIN, i32::MAX - wide as i32),
                _ => (dice.step(200) - 100, dice.step(200) - 100),
            };
            let chunk = |dice: &mut Dice| {
                ChunkPos::new(west + dice.step(long + 1), north + dice.step(wide + 1))
            };
            let mut regions = BTreeSet::new();
            for _ in 0..=dice.below(6) {
                regions.insert(RegionId(dice.below(7) as u32));
            }
            // The home region is one of them three times in four.
            let among: Vec<RegionId> = regions.iter().copied().collect();
            let home = if dice.once_in(4) {
                RegionId(dice.below(7) as u32)
            } else {
                among[dice.below(among.len() as u64) as usize]
            };
            let said = regions
                .into_iter()
                .map(|region| Said {
                    region,
                    fresh: !dice.once_in(3),
                    crowds: (0..dice.below(7))
                        .map(|_| {
                            let players = if dice.once_in(8) {
                                0
                            } else {
                                1 + dice.below(3)
                            };
                            (chunk(&mut dice), players as u32)
                        })
                        .collect(),
                })
                .collect();
            Self {
                policy: policy(merge_distance, split_distance),
                enter: chunk(&mut dice),
                home,
                said,
            }
        }

        fn wanted(&self) -> Vec<Wanted> {
            decide(&self.policy, self.enter, self.home, &sighted(&self.said))
        }

        /// The same world, told in another order.
        fn shuffled(&self, dice: &mut Dice) -> Self {
            let mut other = self.clone();
            dice.shuffle(&mut other.said);
            for said in &mut other.said {
                dice.shuffle(&mut said.crowds);
            }
            other
        }
    }

    /// Whether the seeds are those of every run, and the seeds: 300, or as many as
    /// `CLUSTINE_POLICY_RUNS` says, or the one that `CLUSTINE_POLICY_SEED` names.
    fn seeds() -> (bool, Vec<u64>) {
        let number = |name: &str| {
            std::env::var(name).ok().map(|value| {
                value
                    .parse::<u64>()
                    .unwrap_or_else(|_| panic!("{name} is to be a number, and is {value:?}"))
            })
        };
        match (
            number("CLUSTINE_POLICY_SEED"),
            number("CLUSTINE_POLICY_RUNS"),
        ) {
            (Some(seed), _) => (false, vec![seed]),
            (None, Some(runs)) => (false, (1..=runs).collect()),
            (None, None) => (true, (1..=300).collect()),
        }
    }

    // D11.
    #[test]
    fn any_order_of_the_regions_and_of_the_crowds_of_each_gives_the_same_answer() {
        let (_, seeds) = seeds();
        for seed in seeds {
            let case = Case::of(seed);
            let wanted = case.wanted();
            let mut dice = Dice(!seed);
            for _ in 0..6 {
                let shuffled = case.shuffled(&mut dice);
                assert_eq!(
                    shuffled.wanted(),
                    wanted,
                    "seed {seed} (CLUSTINE_POLICY_SEED={seed}): {case:?}\nand in another order: \
                     {shuffled:?}"
                );
            }
        }
    }

    /// What is wanted, worked out slowly and in the words of section 4: sets of
    /// places that are joined for as long as two of them have places that are
    /// linked, and every statement about a region by going through all of them.
    fn slowly(
        policy: &Policy,
        enter: ChunkPos,
        home: RegionId,
        regions: &[Sighted<'_>],
    ) -> Vec<Wanted> {
        type Spot = (RegionId, ChunkPos);
        let mut players: BTreeMap<Spot, u64> = BTreeMap::new();
        let mut stale = BTreeSet::new();
        let mut sighted = BTreeSet::new();
        for region in regions {
            sighted.insert(region.region);
            if !region.fresh {
                stale.insert(region.region);
            }
            for &(chunk, count) in region.crowds {
                if count > 0 {
                    *players.entry((region.region, chunk)).or_default() += u64::from(count);
                }
            }
        }
        players.entry((home, enter)).or_default();
        let fresh = |region: RegionId| sighted.contains(&region) && !stale.contains(&region);
        let far =
            |one: ChunkPos, other: ChunkPos| one.x.abs_diff(other.x).max(one.z.abs_diff(other.z));
        let linked = |one: &Spot, other: &Spot| {
            if one.0 == other.0 {
                far(one.1, other.1) <= policy.split_distance
            } else {
                far(one.1, other.1) <= policy.merge_distance
            }
        };
        let clusters = |only_known: bool| {
            let mut sets: Vec<BTreeSet<Spot>> = players
                .keys()
                .filter(|spot| !only_known || fresh(spot.0) || **spot == (home, enter))
                .map(|spot| BTreeSet::from([*spot]))
                .collect();
            loop {
                let pair = (0..sets.len())
                    .flat_map(|one| (one + 1..sets.len()).map(move |other| (one, other)))
                    .find(|&(one, other)| {
                        sets[one]
                            .iter()
                            .any(|spot| sets[other].iter().any(|there| linked(spot, there)))
                    });
                let Some((one, other)) = pair else {
                    return sets;
                };
                let joined = sets.remove(other);
                sets[one].extend(joined);
            }
        };
        let (heard, known) = (clusters(false), clusters(true));

        let ids: BTreeSet<RegionId> = players.keys().map(|spot| spot.0).collect();
        let total = |region: RegionId| -> u64 {
            players
                .iter()
                .filter(|(spot, _)| spot.0 == region)
                .map(|(_, count)| count)
                .sum()
        };
        let mut wanted = Vec::new();
        let mut counting: BTreeSet<Spot> = BTreeSet::new();
        for &region in &ids {
            if !fresh(region) {
                continue;
            }
            let of = |set: &BTreeSet<Spot>| -> Vec<ChunkPos> {
                set.iter()
                    .filter(|spot| spot.0 == region)
                    .map(|spot| spot.1)
                    .collect()
            };
            let groups: Vec<Vec<ChunkPos>> = heard
                .iter()
                .map(&of)
                .filter(|group| !group.is_empty())
                .collect();
            let whole = known.iter().filter(|set| !of(set).is_empty()).count() <= 1;
            if groups.len() < 2 {
                if whole {
                    counting.extend(groups.iter().flatten().map(|chunk| (region, *chunk)));
                }
                continue;
            }
            assert!(!whole, "{region} is both whole and apart");
            let count = |group: &Vec<ChunkPos>| -> u64 {
                group.iter().map(|chunk| players[&(region, *chunk)]).sum()
            };
            let stays = if region == home {
                groups.iter().find(|group| group.contains(&enter))
            } else {
                let most = groups.iter().map(count).max();
                groups
                    .iter()
                    .filter(|group| Some(count(group)) == most)
                    .min_by_key(|group| group.iter().min())
            };
            let stays = stays.expect("one group stays");
            counting.extend(stays.iter().map(|chunk| (region, *chunk)));
            let mut go: Vec<Vec<ChunkPos>> = groups
                .iter()
                .filter(|group| *group != stays)
                .cloned()
                .collect();
            for group in &mut go {
                group.sort();
            }
            go.sort_by_key(|group| group[0]);
            wanted.push(Wanted::Split {
                region,
                groups: go,
                why: Why::Apart,
            });
        }
        let mut merges = Vec::new();
        for &lower in &ids {
            for &higher in ids.range(RegionId(lower.0 + 1)..) {
                let gap = counting
                    .iter()
                    .filter(|spot| spot.0 == lower)
                    .flat_map(|spot| {
                        counting
                            .iter()
                            .filter(|there| there.0 == higher)
                            .map(|there| far(spot.1, there.1))
                    })
                    .min();
                if let Some(gap) = gap.filter(|gap| *gap <= policy.merge_distance) {
                    merges.push((gap, lower, higher));
                }
            }
        }
        merges.sort();
        for (gap, lower, higher) in merges {
            let survivor = match [lower, higher].into_iter().find(|region| *region == home) {
                Some(home) => home,
                None if total(higher) > total(lower) => higher,
                None => lower,
            };
            let absorbed = if survivor == lower { higher } else { lower };
            wanted.push(merge(survivor, absorbed, gap));
        }
        wanted
    }

    #[test]
    fn what_is_wanted_of_generated_worlds_is_what_the_rules_give_when_worked_out_slowly() {
        let (every_run, seeds) = seeds();
        let (mut with_split, mut with_merge, mut with_both, mut with_nothing) = (0, 0, 0, 0);
        let mut with_neither = 0;
        for seed in seeds {
            let case = Case::of(seed);
            let wanted = case.wanted();
            let slowly = slowly(&case.policy, case.enter, case.home, &sighted(&case.said));
            assert_eq!(
                wanted, slowly,
                "seed {seed} (CLUSTINE_POLICY_SEED={seed}): {case:?}"
            );
            let splits = wanted
                .iter()
                .filter(|wanted| matches!(wanted, Wanted::Split { .. }))
                .count();
            let merges = wanted.len() - splits;
            with_split += usize::from(splits > 0);
            with_merge += usize::from(merges > 0);
            with_both += usize::from(splits > 0 && merges > 0);
            with_nothing += usize::from(wanted.is_empty());

            // Whether a region is neither surely whole nor surely apart: fresh, with
            // two places that are in one cluster by all that was heard and in two by
            // what is known.
            let places = places(case.enter, case.home, &sighted(&case.said));
            let heard = clusters(&case.policy, &places, |_| true);
            let known = clusters(&case.policy, &places, |place| place.known);
            let neither = places.iter().enumerate().any(|(at, place)| {
                place.fresh
                    && places.iter().enumerate().any(|(other, there)| {
                        there.region == place.region
                            && heard[at] == heard[other]
                            && known[at] != known[other]
                    })
            });
            with_neither += usize::from(neither);
        }
        println!(
            "generated worlds with a split: {with_split}, with a merge: {with_merge}, with \
             both: {with_both}, with nothing wanted: {with_nothing}, with a region that is \
             neither whole nor apart: {with_neither}"
        );
        // The worlds of every run are to be about something.
        if every_run {
            assert!(with_split >= 60, "{with_split}");
            assert!(with_merge >= 60, "{with_merge}");
            assert!(with_both >= 15, "{with_both}");
            assert!(with_nothing >= 30, "{with_nothing}");
            assert!(with_neither >= 15, "{with_neither}");
        }
    }

    #[test]
    fn what_is_wanted_of_generated_worlds_holds_what_the_rules_promise() {
        let (_, seeds) = seeds();
        for seed in seeds {
            let case = Case::of(seed);
            let wanted = case.wanted();
            let told =
                || format!("seed {seed} (CLUSTINE_POLICY_SEED={seed}): {case:?}\n{wanted:?}");
            let fresh = |region: RegionId| {
                case.said
                    .iter()
                    .any(|said| said.region == region && said.fresh)
            };
            let mut splits = Vec::new();
            let mut merges = Vec::new();
            for wanted in &wanted {
                match wanted {
                    Wanted::Split {
                        region,
                        groups,
                        why,
                    } => {
                        // The splits come before the merges.
                        assert!(merges.is_empty(), "{}", told());
                        assert_eq!(*why, Why::Apart, "{}", told());
                        assert!(fresh(*region), "{}", told());
                        assert!(!groups.is_empty(), "{}", told());
                        // Each group ascending, and the groups by their lowest chunks.
                        for group in groups {
                            assert!(!group.is_empty(), "{}", told());
                            assert!(group.is_sorted_by(|one, other| one < other), "{}", told());
                        }
                        assert!(
                            groups.is_sorted_by(|one, other| one[0] < other[0]),
                            "{}",
                            told()
                        );
                        // Any two groups are more than the split distance apart.
                        for (at, group) in groups.iter().enumerate() {
                            for other in &groups[at + 1..] {
                                for (one, there) in group
                                    .iter()
                                    .flat_map(|one| other.iter().map(move |there| (one, there)))
                                {
                                    assert!(
                                        distance(*one, *there)
                                            > u64::from(case.policy.split_distance),
                                        "{}",
                                        told()
                                    );
                                }
                            }
                        }
                        // Who goes is a player the region was said to have, and
                        // never the chunk players enter in.
                        let said = case.said.iter().find(|said| said.region == *region);
                        for chunk in groups.iter().flatten() {
                            assert!(
                                said.is_some_and(|said| said
                                    .crowds
                                    .iter()
                                    .any(|(there, players)| there == chunk && *players > 0)),
                                "{}",
                                told()
                            );
                            assert!(*region != case.home || *chunk != case.enter, "{}", told());
                        }
                        splits.push(*region);
                    }
                    Wanted::Merge {
                        survivor,
                        absorbed,
                        gap,
                        why,
                    } => {
                        assert_eq!(*why, Why::Near, "{}", told());
                        assert_ne!(survivor, absorbed, "{}", told());
                        assert_ne!(*absorbed, case.home, "{}", told());
                        assert!(fresh(*survivor) && fresh(*absorbed), "{}", told());
                        assert!(*gap <= case.policy.merge_distance, "{}", told());
                        merges.push((*gap, *survivor.min(absorbed), *survivor.max(absorbed)));
                    }
                }
            }
            // Each in its order, and nothing twice.
            assert!(splits.is_sorted_by(|one, other| one < other), "{}", told());
            assert!(merges.is_sorted_by(|one, other| one < other), "{}", told());
            let mut pairs: Vec<(RegionId, RegionId)> = merges
                .iter()
                .map(|&(_, lower, higher)| (lower, higher))
                .collect();
            pairs.sort();
            pairs.dedup();
            assert_eq!(pairs.len(), merges.len(), "{}", told());
        }
    }

    /// Measures [`decide`] for a thousand occupied chunks and says how long it took,
    /// to be read with `--nocapture`. It asserts nothing about the time.
    #[test]
    fn a_thousand_occupied_chunks_are_decided_and_how_long_that_takes_is_said() {
        let usual = Policy::for_view_distance(8);
        let region = |number: i32| RegionId(1 + number as u32);
        let measured = |name: &str, said: Vec<Said>| {
            let occupied: usize = said.iter().map(|said| said.crowds.len()).sum();
            let sighted = sighted(&said);
            let began = std::time::Instant::now();
            let wanted = decide(&usual, ORIGIN, HOME, &sighted);
            let took = began.elapsed();
            println!(
                "decide, {name}: {occupied} occupied chunks, {} wanted, {took:?}",
                wanted.len()
            );
            wanted
        };

        // A thousand regions with a player each, none near another: every pair is
        // compared, which is the most there is to compare.
        let scattered = (0..1000)
            .map(|at| fresh(region(at), &[(1000 + at % 40 * 100, at / 40 * 100, 1)]))
            .collect();
        assert_eq!(measured("scattered regions", scattered), nothing());

        // A thousand regions in a row, each at the merge distance from the next.
        let row = (0..1000)
            .map(|at| fresh(region(at), &[(1000 + at * 22, 0, 1)]))
            .collect();
        assert_eq!(measured("a row of regions", row).len(), 999);

        // One region with a thousand chunks side by side.
        let crowd: Vec<(i32, i32, u32)> =
            (0..1000).map(|at| (1000 + at % 40, at / 40, 1)).collect();
        assert_eq!(measured("one crowd", vec![fresh(A, &crowd)]), nothing());

        // Two regions of five hundred chunks each, among each other: every chunk of
        // the one is within the merge distance of chunks of the other.
        let halves = |half: i32| -> Vec<(i32, i32, u32)> {
            (0..1000)
                .filter(|at| at % 2 == half)
                .map(|at| (1000 + at % 40, at / 40, 1))
                .collect()
        };
        let mingled = vec![fresh(A, &halves(0)), fresh(B, &halves(1))];
        assert_eq!(measured("two regions mingled", mingled), [merge(A, B, 1)]);

        // Ten regions of a hundred players each, strewn over the same ground: some
        // are apart, some are near each other.
        let mut dice = Dice(1);
        let strewn: Vec<Said> = (0..10)
            .map(|number| {
                let crowds: Vec<(i32, i32, u32)> = (0..100)
                    .map(|_| (1000 + dice.step(800), dice.step(800), 1))
                    .collect();
                fresh(region(number), &crowds)
            })
            .collect();
        let wanted = measured("ten regions strewn", strewn);
        assert!(!wanted.is_empty());
    }
}
