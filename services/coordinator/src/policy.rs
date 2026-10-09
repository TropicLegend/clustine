//! What a coordinator goes by that merges and splits regions by itself. See
//! `docs/adr/0016-when-to-merge-and-split.md`, sections 3 and 8.
//!
//! Distances are counted in chunks along the longer of the two axes, as the region
//! counts them when it is split.

use std::time::Duration;

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
}
