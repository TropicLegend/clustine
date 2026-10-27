//! A villager's gossip (`GossipContainer`, `GossipType`; `docs/groundwork/n10-brief.md`,
//! section 4.1, its constants confirmed by `n10-dumps.md`, part B, section 1). The
//! contract step N10c gives the store of it; part V3 fills its rules (transfer, decay,
//! reputation events).
//!
//! The game keeps gossips in two hash maps, by target and by type, whose order a
//! transfer's draws read; Clustine keeps them ascending by target, then type (named).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The kinds of gossip, in `GossipType`'s order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum GossipType {
    MajorNegative,
    MinorNegative,
    MinorPositive,
    MajorPositive,
    Trading,
}

impl GossipType {
    /// The name the game saves it by.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::MajorNegative => "major_negative",
            Self::MinorNegative => "minor_negative",
            Self::MinorPositive => "minor_positive",
            Self::MajorPositive => "major_positive",
            Self::Trading => "trading",
        }
    }

    /// `weight`: what a point of it counts in a reputation.
    pub(crate) fn weight(self) -> i32 {
        match self {
            Self::MajorNegative => -5,
            Self::MinorNegative => -1,
            Self::MinorPositive | Self::Trading => 1,
            Self::MajorPositive => 5,
        }
    }

    /// `max`: the most an addition takes it to.
    pub(crate) fn max(self) -> i32 {
        match self {
            Self::MajorNegative => 100,
            Self::MinorNegative => 200,
            Self::MinorPositive | Self::Trading => 25,
            Self::MajorPositive => 20,
        }
    }

    /// `decayPerDay`.
    pub(crate) fn decay_per_day(self) -> i32 {
        match self {
            Self::MajorNegative => 10,
            Self::MinorNegative => 20,
            Self::MinorPositive => 1,
            Self::MajorPositive => 0,
            Self::Trading => 2,
        }
    }

    /// `decayPerTransfer`: what a value loses as it passes to another villager.
    pub(crate) fn decay_per_transfer(self) -> i32 {
        match self {
            Self::MajorNegative => 10,
            Self::MinorNegative => 20,
            Self::MinorPositive => 5,
            Self::MajorPositive | Self::Trading => 20,
        }
    }
}

/// A villager's gossips: for each target and type a value of at least 2, ascending by
/// target and then type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Gossips {
    pub(crate) entries: Vec<(Uuid, GossipType, i32)>,
}

impl Gossips {
    /// `GossipContainer.add(target, type, value)`: the value added, kept at the type's
    /// most unless it was already above it, and gone below 2.
    pub(crate) fn add(&mut self, target: Uuid, kind: GossipType, value: i32) {
        let at = self
            .entries
            .binary_search_by(|(about, of, _)| (*about, *of).cmp(&(target, kind)));
        let old = at.map_or(0, |index| self.entries[index].2);
        let mut new = old.saturating_add(value);
        if new > kind.max() {
            new = kind.max().max(old);
        }
        match (at, new < 2) {
            (Ok(index), true) => {
                self.entries.remove(index);
            }
            (Ok(index), false) => self.entries[index].2 = new,
            (Err(_), true) => {}
            (Err(index), false) => self.entries.insert(index, (target, kind, new)),
        }
    }

    /// `getReputation(target, all)`: the sum of each value by its type's weight.
    pub(crate) fn reputation(&self, target: Uuid) -> i32 {
        self.entries
            .iter()
            .filter(|(about, _, _)| *about == target)
            .map(|(_, kind, value)| value * kind.weight())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_addition_stops_at_the_type_s_most_and_below_two_nothing_is_kept() {
        let mut gossips = Gossips::default();
        let player = Uuid::from_u128(7);
        gossips.add(player, GossipType::MajorNegative, 25);
        gossips.add(player, GossipType::MajorNegative, 90);
        assert_eq!(gossips.entries, [(player, GossipType::MajorNegative, 100)]);
        assert_eq!(gossips.reputation(player), -500);
        gossips.add(player, GossipType::Trading, 1);
        assert_eq!(gossips.entries.len(), 1, "a value below 2 is not kept");
        gossips.add(player, GossipType::Trading, 2);
        assert_eq!(gossips.reputation(player), -498);
    }
}
