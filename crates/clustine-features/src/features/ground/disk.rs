// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/disk.rs`.

//! `disk`: a round patch of a few blocks' height that turns the blocks its target
//! allows into what its provider gives (`DiskFeature`): clay, sand and gravel under
//! water, grass over mud, packed ice.

use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::Feature;
use crate::features::ground::shared::{between_closed, mark_above_for_post_processing};
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

#[derive(Debug)]
pub struct Disk {
    state_provider: Provider,
    target: BlockPredicate,
    radius: IntProvider,
    half_height: i32,
}

impl Disk {
    pub fn read(reader: &Reader) -> Read<Self> {
        let disk = Self {
            state_provider: Provider::read(&reader.field("state_provider")?)?,
            target: BlockPredicate::read(&reader.field("target")?)?,
            radius: IntProvider::read(&reader.field("radius")?)?,
            half_height: reader.field("half_height")?.int()?,
        };
        reader.finish()?;
        Ok(disk)
    }

    /// `placeColumn`: from the top down, every block the target allows takes the
    /// provider's state; the first of each run marks the blocks above it.
    fn place_column(
        &self,
        context: &mut Context<'_, '_>,
        top: i32,
        bottom: i32,
        column: BlockPos,
    ) -> bool {
        let mut placed = false;
        let mut placed_above = false;
        for y in (bottom + 1..=top).rev() {
            let pos = BlockPos::new(column.x, y, column.z);
            if !self.target.test(context, pos) {
                placed_above = false;
                continue;
            }
            // A rule-based provider with nothing to say places nothing.
            if let Some(state) = self.state_provider.optional_state(context, pos) {
                context.level.set_block(pos, state);
                if !placed_above {
                    mark_above_for_post_processing(context.level, pos);
                }
                placed = true;
                placed_above = true;
            }
        }
        placed
    }
}

impl Feature for Disk {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let top = origin.y + self.half_height;
        let bottom = origin.y - self.half_height - 1;
        let radius = self.radius.sample(&mut *context.random);
        let mut placed = false;
        for column in between_closed(
            origin.offset(-radius, 0, -radius),
            origin.offset(radius, 0, radius),
        ) {
            let dx = column.x - origin.x;
            let dz = column.z - origin.z;
            if dx * dx + dz * dz <= radius * radius {
                placed |= self.place_column(context, top, bottom, column);
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

    fn named(name: &str) -> Box<dyn Feature> {
        let (_, entry) = data::configured(name).unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn a_disk_of_clay_replaces_dirt_within_its_radius_and_half_height() {
        let disk = named("minecraft:disk_clay");
        let mut world = World::flat(blocks::DIRT, 60);
        let origin = BlockPos::new(8, 58, 8);
        assert!(world.with_context(3, |context| disk.place(context, origin)));
        assert!(!world.writes.is_empty());
        for (pos, state) in &world.writes {
            assert_eq!(*state, blocks::CLAY);
            let (dx, dz) = (pos.x - 8, pos.z - 8);
            assert!(dx * dx + dz * dz <= 9, "{pos:?}");
            assert!((57..=59).contains(&pos.y), "{pos:?}");
        }
        // Every column is a full run under dirt: its top marks the two blocks above.
        assert!(world.marks.contains(&BlockPos::new(8, 60, 8)));
        assert!(!world.marks.contains(&BlockPos::new(8, 61, 8)));
    }

    #[test]
    fn a_disk_of_grass_takes_grass_where_nothing_covers_it_and_dirt_below() {
        let disk = named("minecraft:disk_grass");
        let mut world = World::flat(blocks::DIRT, 60);
        world.set(BlockPos::new(8, 60, 8), clustine_data::blocks::MUD);
        assert!(world.with_context(11, |context| disk.place(context, BlockPos::new(8, 60, 8))));
        let top = world
            .writes
            .iter()
            .find(|(pos, _)| *pos == BlockPos::new(8, 60, 8))
            .map(|(_, state)| *state)
            .unwrap();
        assert!(crate::blocks::same_block(top, blocks::GRASS_BLOCK));
        let below = world
            .writes
            .iter()
            .find(|(pos, _)| *pos == BlockPos::new(8, 59, 8))
            .map(|(_, state)| *state)
            .unwrap();
        assert_eq!(below, blocks::DIRT);
    }

    #[test]
    fn a_disk_where_its_target_is_missing_places_nothing() {
        let disk = named("minecraft:disk_clay");
        let mut world = World::flat(blocks::STONE, 60);
        assert!(!world.with_context(3, |context| disk.place(context, BlockPos::new(8, 58, 8))));
        assert!(world.writes.is_empty() && world.marks.is_empty());
    }
}
