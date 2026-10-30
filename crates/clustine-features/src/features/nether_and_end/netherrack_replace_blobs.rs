//! `netherrack_replace_blobs`: a blob of one block that replaces another, found by
//! going down from the origin (`ReplaceBlobsFeature`, read from
//! `d-javap/net.minecraft.world.level.levelgen.feature.ReplaceBlobsFeature.txt`; the
//! offsets below are its bytecode's): the basalt deltas' blobs of basalt and
//! blackstone in netherrack.

use clustine_data::BlockState;
use clustine_world::BlockPos;

use super::shared::{manhattan, within_manhattan};
use crate::blocks::{read_state, same_block};
use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

#[derive(Debug)]
pub struct NetherrackReplaceBlobs {
    target: BlockState,
    state: BlockState,
    radius: IntProvider,
}

impl NetherrackReplaceBlobs {
    pub fn read(reader: &Reader) -> Read<Self> {
        let blobs = Self {
            target: read_state(&reader.field("target")?)?,
            state: read_state(&reader.field("state")?)?,
            radius: IntProvider::read(&reader.field("radius")?)?,
        };
        reader.finish()?;
        Ok(blobs)
    }

    /// `findTarget` (offsets 0-45): down from `cursor` to the first block of the
    /// target, while above the second block from the bottom, which is never looked at.
    fn find_target(&self, context: &mut Context<'_, '_>, mut cursor: BlockPos) -> Option<BlockPos> {
        while cursor.y > context.level.min_y() + 1 {
            if same_block(context.level.block(cursor), self.target) {
                return Some(cursor);
            }
            cursor = cursor.offset(0, -1, 0);
        }
        None
    }
}

impl Feature for NetherrackReplaceBlobs {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        // The origin is first brought within the level's height, one above its floor
        // and at most its highest block (`clamp(Axis.Y, getMinY() + 1, getMaxY())`,
        // 9-32): the level's, which in the Nether is all 256 blocks, not the
        // generator's 128. No target, nothing placed and nothing drawn (42-48).
        let y = origin
            .y
            .clamp(context.level.min_y() + 1, context.level.top_y() - 1);
        let Some(centre) = self.find_target(context, BlockPos::new(origin.x, y, origin.z)) else {
            return false;
        };
        // Three radii, x, y and z in that order (49-83), and no block farther from the
        // centre than the greatest of them (85-97). The walk is
        // `withinBoxByManhattanDistance` (102-118), nearest first, which stops at the
        // first block beyond that distance (142-154); each block of the target is
        // replaced (157-189), and the answer is whether one was (194).
        let radius_x = self.radius.sample(context.random);
        let radius_y = self.radius.sample(context.random);
        let radius_z = self.radius.sample(context.random);
        let greatest = radius_x.max(radius_y).max(radius_z);
        let mut placed = false;
        for pos in within_manhattan(centre, radius_x, radius_y, radius_z) {
            if manhattan(pos, centre) > greatest {
                break;
            }
            if same_block(context.level.block(pos), self.target) {
                context.level.set_block(pos, self.state);
                placed = true;
            }
        }
        placed
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn blobs(name: &str) -> Box<dyn Feature> {
        let (_, entry) = data::configured(name).unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn a_blob_of_basalt_replaces_netherrack_within_a_diamond_below_the_origin() {
        let blobs = blobs("minecraft:basalt_blobs");
        let mut world = World::flat(blocks::NETHERRACK, 40);
        assert!(world.with_context(3, |context| blobs.place(context, BlockPos::new(8, 90, 8))));
        assert!(!world.writes.is_empty());
        // The centre is the netherrack found going down: the ground's top.
        let centre = BlockPos::new(8, 40, 8);
        assert!(world.writes.contains(&(centre, blocks::BASALT)));
        for (pos, state) in &world.writes {
            assert_eq!(*state, blocks::BASALT);
            // Only netherrack is replaced, and the radii are 3 to 7.
            assert!(pos.y <= 40, "{pos:?}");
            assert!(manhattan(*pos, centre) <= 7, "{pos:?}");
        }
    }

    #[test]
    fn a_blob_replaces_only_its_target_and_draws_its_three_radii_in_order() {
        let blobs = blobs("minecraft:blackstone_blobs");
        let mut world = World::flat(blocks::NETHERRACK, 40);
        // A pillar of another block in the blob is left as it is.
        for y in 30..=40 {
            world.set(BlockPos::new(9, y, 8), blocks::SOUL_SAND);
        }
        let mut drawn = clustine_noise::WorldgenRandom::from_seed(11);
        let radii: Vec<i32> = (0..3)
            .map(|_| clustine_noise::Random::next_i32_bounded(&mut drawn, 5) + 3)
            .collect();
        let greatest = *radii.iter().max().unwrap();
        assert!(world.with_context(11, |context| blobs.place(context, BlockPos::new(8, 40, 8))));
        let centre = BlockPos::new(8, 40, 8);
        let expected: usize = within_manhattan(centre, radii[0], radii[1], radii[2])
            .filter(|pos| manhattan(*pos, centre) <= greatest)
            .filter(|pos| pos.y <= 40 && !(pos.x == 9 && pos.z == 8 && pos.y >= 30))
            .filter(|pos| pos.chunk().x.abs() <= 1 && pos.chunk().z.abs() <= 1)
            .count();
        assert_eq!(world.writes.len(), expected);
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| *state == blocks::BLACKSTONE)
        );
    }

    #[test]
    fn a_blob_over_no_target_places_nothing() {
        let blobs = blobs("minecraft:basalt_blobs");
        let mut world = World::flat(blocks::STONE, 40);
        assert!(!world.with_context(3, |context| blobs.place(context, BlockPos::new(8, 90, 8))));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn an_origin_above_the_level_is_brought_down_to_its_highest_block() {
        use super::super::shared::with_nether_context;

        let blobs = blobs("minecraft:basalt_blobs");
        // Netherrack up to the Nether's top: the search starts at 255, not at 400.
        let mut world = World::flat(blocks::NETHERRACK, 255);
        assert!(with_nether_context(&mut world, 3, |context| {
            blobs.place(context, BlockPos::new(8, 400, 8))
        }));
        assert!(
            world
                .writes
                .contains(&(BlockPos::new(8, 255, 8), blocks::BASALT))
        );
        assert!(world.writes.iter().all(|(pos, _)| pos.y <= 255));
    }

    #[test]
    fn the_search_never_looks_at_the_second_block_from_the_bottom() {
        use super::super::shared::with_nether_context;

        let blobs = blobs("minecraft:basalt_blobs");
        // Netherrack only at y 1, one above the Nether's floor: never found.
        let mut world = World::flat(blocks::STONE, 0);
        world.set(BlockPos::new(8, 1, 8), blocks::NETHERRACK);
        assert!(!with_nether_context(&mut world, 3, |context| {
            blobs.place(context, BlockPos::new(8, 60, 8))
        }));
        // At y 2 it is.
        world.set(BlockPos::new(8, 2, 8), blocks::NETHERRACK);
        assert!(with_nether_context(&mut world, 3, |context| {
            blobs.place(context, BlockPos::new(8, 60, 8))
        }));
    }
}
