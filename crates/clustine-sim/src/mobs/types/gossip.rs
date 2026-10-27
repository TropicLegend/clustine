//! A villager's gossip (`GossipContainer`, `GossipType`; `docs/groundwork/n10-brief.md`,
//! section 4.1, its constants confirmed by `n10-dumps.md`, part B, section 1): what it
//! adds, what passes from one villager to another (`transferFrom`, ten draws of the
//! receiver's own stream), the day's decay, and what each reputation event writes
//! (`Villager.onReputationEventFrom`).
//!
//! The game keeps gossips in two hash maps, by target and by type, whose order a
//! transfer's draws read; Clustine keeps them ascending by target, then type (named:
//! which entry a draw picks is so a function of the gossips, as the game's is a function
//! of its hashes, and is judged by distribution).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::region::Random;

/// `GossipContainer.DISCARD_THRESHOLD`: a value below it is not kept.
const DISCARD_THRESHOLD: i32 = 2;

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

/// `ReputationEventType`, in the order the game registers them. `GolemKilled` is raised
/// nowhere and writes nothing (`n10-dumps.md`, part B, correction 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReputationEvent {
    ZombieVillagerCured,
    #[allow(dead_code, reason = "the game raises it nowhere")]
    GolemKilled,
    VillagerHurt,
    VillagerKilled,
    Trade,
}

/// A villager's gossips: for each target and type a value of at least 2, ascending by
/// target and then type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Gossips {
    pub(crate) entries: Vec<(Uuid, GossipType, i32)>,
}

impl Gossips {
    /// The place of the entry of `target` and `kind`, or where it would go.
    fn find(&self, target: Uuid, kind: GossipType) -> Result<usize, usize> {
        self.entries
            .binary_search_by(|(about, of, _)| (*about, *of).cmp(&(target, kind)))
    }

    /// `GossipContainer.add(target, type, value)`: the value added, never above the
    /// type's most, and gone below 2.
    pub(crate) fn add(&mut self, target: Uuid, kind: GossipType, value: i32) {
        let at = self.find(target, kind);
        let old = at.map_or(0, |index| self.entries[index].2);
        // `mergeValuesForAddition` keeps an old value that is above the most, and
        // `makeSureValueIsntTooLowOrTooHigh` then brings it down to the most.
        let new = old.saturating_add(value).min(kind.max());
        match (at, new < DISCARD_THRESHOLD) {
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
        self.reputation_of(target, |_| true)
    }

    /// `getReputation(target, types)`: the sum over the types `types` passes.
    pub(crate) fn reputation_of(&self, target: Uuid, types: impl Fn(GossipType) -> bool) -> i32 {
        self.entries
            .iter()
            .filter(|(about, kind, _)| *about == target && types(*kind))
            .map(|(_, kind, value)| value.wrapping_mul(kind.weight()))
            .fold(0, i32::wrapping_add)
    }

    /// `GossipContainer.decay()`: each value less its type's decay of a day, gone below
    /// 2.
    pub(crate) fn decay(&mut self) {
        self.entries.retain_mut(|(_, kind, value)| {
            *value -= kind.decay_per_day();
            *value >= DISCARD_THRESHOLD
        });
    }

    /// `transferFrom(other, random, count)` into these gossips, the receiver's, from
    /// `other`'s: `count` draws of `nextInt(total)` from the receiver's stream, where
    /// `total` is the sum of every entry's `|value × weight|`, each finding its entry by
    /// the cumulative weights less one (`Arrays.binarySearch`, or its insertion point);
    /// an entry picked twice counts once. Each entry picked arrives as its value less its
    /// type's decay of a transfer where that is at least 2, kept as the higher of it and
    /// what the receiver had (`mergeValuesForTransfer`, which brings nothing down to the
    /// most). How many entries were picked; nothing is drawn where `other` has none.
    pub(crate) fn transfer_from(
        &mut self,
        other: &Gossips,
        random: &mut Random,
        count: u32,
    ) -> u32 {
        if other.entries.is_empty() {
            return 0;
        }
        let mut cumulative = Vec::with_capacity(other.entries.len());
        let mut total: i32 = 0;
        for (_, kind, value) in &other.entries {
            total = total.wrapping_add(value.wrapping_mul(kind.weight()).wrapping_abs());
            cumulative.push(total.wrapping_sub(1));
        }
        let mut picked: Vec<usize> = Vec::new();
        for _ in 0..count {
            let drawn = random.next_int(u32::try_from(total).unwrap_or(1)) as i32;
            // The weights are each at least 2, so the list rises strictly: the first
            // entry whose cumulative weight less one is at least the draw is the one
            // `binarySearch` finds, or the insertion point it gives.
            let index = cumulative.partition_point(|at| *at < drawn);
            if index < other.entries.len() && !picked.contains(&index) {
                picked.push(index);
            }
        }
        // The game's set is an identity set, iterated in its own order; each entry is
        // of its own target and type, so the order of the merges changes nothing.
        for &index in &picked {
            let (target, kind, value) = other.entries[index];
            let arrives = value - kind.decay_per_transfer();
            if arrives < DISCARD_THRESHOLD {
                continue;
            }
            match self.find(target, kind) {
                Ok(at) => self.entries[at].2 = self.entries[at].2.max(arrives),
                Err(at) => self.entries.insert(at, (target, kind, arrives)),
            }
        }
        picked.len() as u32
    }

    /// `Villager.onReputationEventFrom(event, entity)`'s gossips about the entity of
    /// UUID `about`: a cure's major and minor positive, a trade's trading, a hurt's minor
    /// negative, a murder's major negative; a golem's death nothing.
    pub(crate) fn reputation_event(&mut self, event: ReputationEvent, about: Uuid) {
        match event {
            ReputationEvent::ZombieVillagerCured => {
                self.add(about, GossipType::MajorPositive, 20);
                self.add(about, GossipType::MinorPositive, 25);
            }
            ReputationEvent::Trade => self.add(about, GossipType::Trading, 2),
            ReputationEvent::VillagerHurt => self.add(about, GossipType::MinorNegative, 25),
            ReputationEvent::VillagerKilled => self.add(about, GossipType::MajorNegative, 25),
            ReputationEvent::GolemKilled => {}
        }
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
