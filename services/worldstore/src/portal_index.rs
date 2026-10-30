//! The portal index (`docs/groundwork/p-plan.md`, decision 8 and sections 3.3 and 15.4):
//! where the `nether_portal` blocks of each dimension are, kept from the block changes
//! of every commit the store takes, so that a destination finds the portals of land it
//! has never loaded as the game's point-of-interest search finds them
//! (`PortalForcer.findClosestPortalPosition`, `getInSquare`).
//!
//! It is as durable as the log and as current as the last commit: a commit's changes
//! are applied when the store takes the commit, and a restart builds the index again
//! from what its checkpoint kept and the commits after it.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, Change, Dimension};

/// The portal blocks of every dimension.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PortalIndex {
    /// By dimension, each block as (x, z, y), so that a square on x and z is a range
    /// of x.
    blocks: BTreeMap<Dimension, BTreeSet<(i32, i32, i32)>>,
}

impl PortalIndex {
    /// Takes the block changes of one commit of a region of `dimension`, in their
    /// order: a position that is a portal after them is in the index, any other is not.
    /// Returns whether the index changed, which the table file has to be written for
    /// before the log that says it can be let go.
    pub(crate) fn apply(
        &mut self,
        dimension: Dimension,
        changes: &[(BlockPos, BlockState)],
    ) -> bool {
        let mut changed = false;
        for (position, state) in changes {
            let key = (position.x, position.z, position.y);
            if is_portal(*state) {
                changed |= self.blocks.entry(dimension).or_default().insert(key);
            } else if let Some(of_dimension) = self.blocks.get_mut(&dimension) {
                changed |= of_dimension.remove(&key);
            }
        }
        self.blocks
            .retain(|_, of_dimension| !of_dimension.is_empty());
        changed
    }

    /// The portal blocks of `dimension` within `radius` blocks of `centre` on x and z,
    /// every height, ascending by x, z and then y: the game's `getInSquare` around the
    /// target, which compares the found ones afterwards.
    pub(crate) fn in_square(
        &self,
        dimension: Dimension,
        centre: BlockPos,
        radius: i32,
    ) -> Vec<BlockPos> {
        let Some(of_dimension) = self.blocks.get(&dimension) else {
            return Vec::new();
        };
        let radius = radius.max(0);
        let (low_x, high_x) = (
            centre.x.saturating_sub(radius),
            centre.x.saturating_add(radius),
        );
        let (low_z, high_z) = (
            centre.z.saturating_sub(radius),
            centre.z.saturating_add(radius),
        );
        of_dimension
            .range((low_x, i32::MIN, i32::MIN)..=(high_x, i32::MAX, i32::MAX))
            .filter(|(_, z, _)| (low_z..=high_z).contains(z))
            .map(|&(x, z, y)| BlockPos::new(x, y, z))
            .collect()
    }

    /// Every portal block, by dimension: what a checkpoint keeps.
    pub(crate) fn all(&self) -> impl Iterator<Item = (Dimension, BlockPos)> + '_ {
        self.blocks.iter().flat_map(|(dimension, of_dimension)| {
            of_dimension
                .iter()
                .map(move |&(x, z, y)| (*dimension, BlockPos::new(x, y, z)))
        })
    }

    /// The index of these blocks, as a checkpoint kept them.
    pub(crate) fn of(kept: impl IntoIterator<Item = (Dimension, BlockPos)>) -> Self {
        let mut index = Self::default();
        for (dimension, position) in kept {
            index
                .blocks
                .entry(dimension)
                .or_default()
                .insert((position.x, position.z, position.y));
        }
        index
    }
}

/// The block changes among a commit's changes, in their order.
pub(crate) fn blocks_of(changes: &[Change]) -> Vec<(BlockPos, BlockState)> {
    changes
        .iter()
        .filter_map(|change| match change {
            Change::Block(position, state) => Some((*position, *state)),
            Change::BlockEntity(..) => None,
        })
        .collect()
}

/// Whether `state` is a `nether_portal` block, of either axis.
fn is_portal(state: BlockState) -> bool {
    state.block_id().is_some() && state.block_id() == blocks::NETHER_PORTAL.block_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nether portal's state along the other axis than the default's.
    fn other_axis() -> BlockState {
        let portal = blocks::NETHER_PORTAL;
        (0..4)
            .map(|offset| BlockState(portal.0.wrapping_add(offset).wrapping_sub(1)))
            .find(|state| *state != portal && is_portal(*state))
            .expect("a nether portal has two states")
    }

    #[test]
    fn a_portal_block_of_either_axis_is_kept_and_anything_put_in_its_place_takes_it_out() {
        let mut index = PortalIndex::default();
        let (a, b) = (BlockPos::new(3, 70, -5), BlockPos::new(3, 71, -5));
        index.apply(
            Dimension::Overworld,
            &[(a, blocks::NETHER_PORTAL), (b, other_axis())],
        );
        assert_eq!(
            index.in_square(Dimension::Overworld, BlockPos::new(0, 0, 0), 16),
            [a, b]
        );
        index.apply(
            Dimension::Overworld,
            &[(a, blocks::AIR), (b, blocks::OBSIDIAN)],
        );
        assert_eq!(
            index.in_square(Dimension::Overworld, BlockPos::new(0, 0, 0), 16),
            []
        );
        assert_eq!(index, PortalIndex::default());
    }

    #[test]
    fn the_last_of_a_commits_changes_to_a_position_decides() {
        let mut index = PortalIndex::default();
        let at = BlockPos::new(0, 64, 0);
        index.apply(
            Dimension::Nether,
            &[(at, blocks::NETHER_PORTAL), (at, blocks::AIR)],
        );
        assert_eq!(index.in_square(Dimension::Nether, at, 1), []);
        index.apply(
            Dimension::Nether,
            &[(at, blocks::AIR), (at, blocks::NETHER_PORTAL)],
        );
        assert_eq!(index.in_square(Dimension::Nether, at, 1), [at]);
    }

    #[test]
    fn a_square_takes_every_height_and_both_edges_and_nothing_beyond_them() {
        let mut index = PortalIndex::default();
        let inside = [
            BlockPos::new(-16, 0, -16),
            BlockPos::new(16, 255, 16),
            BlockPos::new(0, 128, 16),
            BlockPos::new(-16, -64, 0),
        ];
        let outside = [
            BlockPos::new(17, 64, 0),
            BlockPos::new(0, 64, -17),
            BlockPos::new(-17, 64, 17),
        ];
        let changes: Vec<_> = inside
            .iter()
            .chain(&outside)
            .map(|position| (*position, blocks::NETHER_PORTAL))
            .collect();
        index.apply(Dimension::Overworld, &changes);
        let mut expected = inside.to_vec();
        expected.sort_by_key(|position| (position.x, position.z, position.y));
        assert_eq!(
            index.in_square(Dimension::Overworld, BlockPos::new(0, 70, 0), 16),
            expected
        );
    }

    #[test]
    fn the_dimensions_are_kept_apart() {
        let mut index = PortalIndex::default();
        let at = BlockPos::new(8, 70, 8);
        index.apply(Dimension::Nether, &[(at, blocks::NETHER_PORTAL)]);
        assert_eq!(index.in_square(Dimension::Overworld, at, 128), []);
        assert_eq!(index.in_square(Dimension::End, at, 128), []);
        assert_eq!(index.in_square(Dimension::Nether, at, 0), [at]);
        // Taking out the overworld's block of the same position leaves the Nether's.
        index.apply(Dimension::Overworld, &[(at, blocks::AIR)]);
        assert_eq!(index.in_square(Dimension::Nether, at, 0), [at]);
    }

    #[test]
    fn what_a_checkpoint_keeps_is_the_index_again() {
        let mut index = PortalIndex::default();
        index.apply(
            Dimension::Overworld,
            &[(BlockPos::new(1, 2, 3), blocks::NETHER_PORTAL)],
        );
        index.apply(
            Dimension::Nether,
            &[(BlockPos::new(-4, 5, -6), blocks::NETHER_PORTAL)],
        );
        let kept: Vec<_> = index.all().collect();
        assert_eq!(kept.len(), 2);
        assert_eq!(PortalIndex::of(kept), index);
    }

    #[test]
    fn a_square_at_the_edge_of_the_numbers_does_not_overflow() {
        let mut index = PortalIndex::default();
        let far = BlockPos::new(i32::MAX, 64, i32::MIN);
        index.apply(Dimension::Overworld, &[(far, blocks::NETHER_PORTAL)]);
        assert_eq!(index.in_square(Dimension::Overworld, far, 128), [far]);
        assert_eq!(index.in_square(Dimension::Overworld, far, -1), [far]);
    }
}
