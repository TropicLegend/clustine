// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/freeze_top_layer.rs`
// and `steel-core/src/worldgen/feature/weather.rs`.

//! `freeze_top_layer`: over the sixteen by sixteen columns from the origin, water at
//! the top turns to ice and snow falls where it is cold, as the weather would do
//! (`SnowAndFreezeFeature`). The block under new snow turns snowy where it can.

use clustine_data::blocks;
use clustine_world::BlockPos;

use crate::context::{Context, Heightmap};
use crate::feature::Feature;
use crate::features::ground::shared::{should_freeze, should_snow};
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct FreezeTopLayer;

impl FreezeTopLayer {
    pub fn read(reader: &Reader) -> Read<Self> {
        reader.finish()?;
        Ok(Self)
    }
}

impl Feature for FreezeTopLayer {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let sea_level = context.settings.generation().sea_level;
        let level = &mut *context.level;
        for dx in 0..16 {
            for dz in 0..16 {
                let (x, z) = (origin.x + dx, origin.z + dz);
                let y = level.height_at(Heightmap::MotionBlocking, x, z);
                let top = BlockPos::new(x, y, z);
                let below = BlockPos::new(x, y - 1, z);
                let biome = level.biome(top);
                if should_freeze(level, biome, below, sea_level, false) {
                    level.set_block(below, blocks::ICE);
                }
                if should_snow(level, biome, top, sea_level) {
                    level.set_block(top, blocks::SNOW);
                    let under = level.block(below);
                    if let Some(snowy) = under.with_property("snowy", "true") {
                        level.set_block(below, snowy);
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_world::Biome;
    use clustine_worldgen_data::biomes::CLIMATE;

    use super::*;
    use crate::testing::World;

    fn biome(name: &str) -> Biome {
        let suffix = name.trim_start_matches("minecraft:");
        let index = CLIMATE
            .iter()
            .position(|climate| climate.name.trim_start_matches("minecraft:") == suffix)
            .unwrap();
        Biome(index as u16)
    }

    #[test]
    fn in_the_snowy_plains_water_freezes_and_grass_turns_snowy_under_snow() {
        let mut world = World::flat(blocks::GRASS_BLOCK, 62);
        world.biome = biome("minecraft:snowy_plains");
        world.set(BlockPos::new(3, 62, 4), blocks::WATER);
        assert!(world.with_context(0, |context| {
            FreezeTopLayer.place(context, BlockPos::new(0, 0, 0))
        }));
        assert_eq!(world.get(BlockPos::new(3, 62, 4)), blocks::ICE);
        // Snow does not lie on ice.
        assert_eq!(world.get(BlockPos::new(3, 63, 4)), blocks::AIR);
        assert_eq!(world.get(BlockPos::new(5, 63, 5)), blocks::SNOW);
        assert_eq!(
            world.get(BlockPos::new(5, 62, 5)).property("snowy"),
            Some("true")
        );
        // Sixteen by sixteen columns from the origin, and nothing beyond.
        assert_eq!(world.get(BlockPos::new(16, 63, 5)), blocks::AIR);
        assert!(world.writes.iter().all(|(pos, _)| pos.x < 16 && pos.z < 16));
        assert!(world.ticks.is_empty() && world.marks.is_empty());
    }

    #[test]
    fn in_the_plains_nothing_freezes_below_the_snow_line() {
        let mut world = World::flat(blocks::WATER, 62);
        world.biome = biome("minecraft:plains");
        world.with_context(0, |context| {
            FreezeTopLayer.place(context, BlockPos::new(0, 0, 0))
        });
        assert!(world.writes.is_empty());
    }

    #[test]
    fn flowing_water_does_not_freeze() {
        let mut world = World::flat(blocks::STONE, 61);
        world.biome = biome("minecraft:frozen_river");
        let flowing = blocks::WATER.with_property("level", "3").unwrap();
        world.set(BlockPos::new(1, 62, 1), flowing);
        world.with_context(0, |context| {
            FreezeTopLayer.place(context, BlockPos::new(0, 0, 0))
        });
        assert_eq!(world.get(BlockPos::new(1, 62, 1)), flowing);
    }
}
