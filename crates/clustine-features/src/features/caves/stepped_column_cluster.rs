// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/stepped_column_cluster.rs`.

//! `stepped_column_cluster`: a cluster of columns that step down from its centre, each
//! a small square of columns of one block standing on the ground or filling the space
//! up from it (`SteppedColumnClusterFeature`, 26.x's basalt columns of the Nether made
//! general: `small_basalt_columns` and `large_basalt_columns`).

use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::BlockSet;
use crate::context::Context;
use crate::feature::Feature;
use crate::features::ground::shared::between_closed;
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

#[derive(Debug)]
pub struct SteppedColumnCluster {
    block: Provider,
    continue_through: BlockPredicate,
    can_replace: BlockPredicate,
    cannot_place_on: BlockSet,
    cluster_reach: IntProvider,
    column_count: IntProvider,
    column_reach: IntProvider,
    height: IntProvider,
}

fn manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.x - b.x).abs() + (a.y - b.y).abs() + (a.z - b.z).abs()
}

impl SteppedColumnCluster {
    pub fn read(reader: &Reader) -> Read<Self> {
        let int = |key: &str| IntProvider::read(&reader.field(key)?);
        let cluster = Self {
            block: Provider::read(&reader.field("block")?)?,
            continue_through: BlockPredicate::read(&reader.field("continue_through")?)?,
            can_replace: BlockPredicate::read(&reader.field("can_replace")?)?,
            cannot_place_on: BlockSet::read(&reader.field("cannot_place_on")?)?,
            cluster_reach: int("cluster_reach")?,
            column_count: int("column_count")?,
            column_reach: int("column_reach")?,
            height: int("height")?,
        };
        reader.finish()?;
        Ok(cluster)
    }

    /// Whether a column may stand at `pos`: on something that is not air and not
    /// what it may not stand on.
    fn can_place_at(&self, context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
        if !self.can_replace.test(context, pos) {
            return false;
        }
        let below = context.level.block(pos.offset(0, -1, 0));
        !below.props().is_air() && !self.cannot_place_on.contains(below)
    }

    /// Down from `pos` to the first place a column may stand, within `limit` blocks.
    fn find_surface(
        &self,
        context: &mut Context<'_, '_>,
        mut pos: BlockPos,
        mut limit: i32,
    ) -> Option<BlockPos> {
        while pos.y > context.level.min_y() + 1 && limit > 0 {
            limit -= 1;
            if self.can_place_at(context, pos) {
                return Some(pos);
            }
            pos = pos.offset(0, -1, 0);
        }
        None
    }

    /// Up from `pos` to the first air, within `limit` blocks, unless what it may not
    /// stand on comes first.
    fn find_air(
        &self,
        context: &mut Context<'_, '_>,
        mut pos: BlockPos,
        mut limit: i32,
    ) -> Option<BlockPos> {
        while pos.y < context.level.top_y() && limit > 0 {
            limit -= 1;
            let state = context.level.block(pos);
            if self.cannot_place_on.contains(state) {
                return None;
            }
            if state.props().is_air() {
                return Some(pos);
            }
            pos = pos.offset(0, 1, 0);
        }
        None
    }

    /// One stepped column: a square of `reach` about `origin`, each block of it a
    /// column whose height falls with its distance from the middle.
    fn place_column(
        &self,
        context: &mut Context<'_, '_>,
        origin: BlockPos,
        height: i32,
        reach: i32,
    ) -> bool {
        let mut placed = false;
        let min = origin.offset(-reach, 0, -reach);
        let max = origin.offset(reach, 0, reach);
        for pos in between_closed(min, max) {
            let distance = manhattan(pos, origin);
            let start = if self.can_replace.test(context, pos) {
                self.find_surface(context, pos, distance)
            } else {
                self.find_air(context, pos, distance)
            };
            let Some(mut cursor) = start else {
                continue;
            };
            let mut left = height - distance / 2;
            while left >= 0 {
                if self.can_replace.test(context, cursor) {
                    let state = self.block.state(context, cursor);
                    context.level.set_block(cursor, state);
                    placed = true;
                } else if !self.continue_through.test(context, cursor) {
                    break;
                }
                cursor = cursor.offset(0, 1, 0);
                left -= 1;
            }
        }
        placed
    }
}

impl Feature for SteppedColumnCluster {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !self.can_place_at(context, origin) {
            return false;
        }
        let height = self.height.sample(&mut *context.random);
        let reach = height.min(self.cluster_reach.sample(&mut *context.random));
        let count = self.column_count.sample(&mut *context.random);
        let mut placed = false;
        for _ in 0..count {
            let random = &mut *context.random;
            let x = origin.x - reach + random.next_i32_bounded(reach * 2 + 1);
            // The game draws a height of at most nought here too.
            let y = origin.y + random.next_i32_bounded(1);
            let z = origin.z - reach + random.next_i32_bounded(reach * 2 + 1);
            let pos = BlockPos::new(x, y, z);
            let blocks = height - manhattan(pos, origin);
            if blocks >= 0 {
                let column_reach = self.column_reach.sample(&mut *context.random);
                placed |= self.place_column(context, pos, blocks, column_reach);
            }
        }
        placed
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::blocks::same_block;
    use crate::testing::World;

    /// The first of the data's `small_basalt_columns`, read from the data.
    fn small_basalt_columns() -> SteppedColumnCluster {
        let (_, entry) =
            clustine_worldgen_data::feature::configured("minecraft:small_basalt_columns").unwrap();
        let Some(Data::List(features)) = entry.data.get("features") else {
            panic!("a list");
        };
        let feature = features[0].get("data").unwrap().get("feature").unwrap();
        let reader = Reader::new(feature, "test");
        reader.kind().unwrap();
        SteppedColumnCluster::read(&reader).unwrap()
    }

    #[test]
    fn basalt_columns_stand_on_netherrack_and_rise_from_the_middle_out() {
        let columns = small_basalt_columns();
        let mut world = World::flat(blocks::NETHERRACK, 40);
        assert!(world.with_context(4, |context| {
            columns.place(context, BlockPos::new(8, 41, 8))
        }));
        assert!(!world.writes.is_empty());
        assert!(
            world
                .writes
                .iter()
                .all(|(pos, state)| same_block(*state, blocks::BASALT) && pos.y >= 41)
        );
        assert!(
            world
                .writes
                .iter()
                .all(|(pos, _)| (pos.x - 8).abs() <= 8 && (pos.z - 8).abs() <= 8)
        );
    }

    #[test]
    fn columns_are_not_placed_on_what_they_may_not_stand_on() {
        let columns = small_basalt_columns();
        let mut world = World::flat(blocks::BEDROCK, 40);
        assert!(!world.with_context(4, |context| {
            columns.place(context, BlockPos::new(8, 41, 8))
        }));
        assert!(world.writes.is_empty());
    }
}
