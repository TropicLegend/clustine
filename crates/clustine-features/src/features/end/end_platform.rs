//! `end_platform`: the five by five of obsidian a traveller into the End stands on,
//! with three blocks of air over it (`EndPlatformFeature`, read from `d-javap`; the
//! offsets below are its bytecode's). The End's middle biome places it at
//! (100, 49, 0); an arrival through the End portal makes it again
//! (`createEndPlatform` with drops, the plan's P10b), for which [`platform_blocks`] is
//! the shape.

use clustine_data::{BlockState, blocks};
use clustine_world::BlockPos;

use crate::blocks::same_block;
use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};

/// Where the game puts the platform, the block under an arrival's feet
/// (`END_SPAWN_POINT` below one, and the placed feature's position).
pub const END_PLATFORM: BlockPos = BlockPos::new(100, 49, 0);

/// The platform's blocks about `origin`, in the order `createEndPlatform` sets them
/// (offsets 0-130): for each z from −2 to 2, each x from −2 to 2, obsidian one below
/// `origin`'s level and air on it and the two above.
pub fn platform_blocks(origin: BlockPos) -> impl Iterator<Item = (BlockPos, BlockState)> {
    (-2..=2).flat_map(move |dz| {
        (-2..=2).flat_map(move |dx| {
            (-1..3).map(move |dy| {
                let state = if dy == -1 {
                    blocks::OBSIDIAN
                } else {
                    blocks::AIR
                };
                (origin.offset(dx, dy, dz), state)
            })
        })
    })
}

#[derive(Debug)]
pub struct EndPlatform;

impl EndPlatform {
    /// The kind has no settings (`MapCodec.unit`).
    pub fn read(reader: &Reader) -> Read<Self> {
        reader.finish()?;
        Ok(Self)
    }
}

impl Feature for EndPlatform {
    /// `place` (offsets 0-8): `createEndPlatform` without drops. Each block is set
    /// only where the block there is of another kind (`BlockState.is(Block)`, offsets
    /// 67-80).
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        for (at, state) in platform_blocks(origin) {
            if !same_block(context.level.block(at), state) {
                context.level.set_block(at, state);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::World;

    #[test]
    fn the_platform_is_obsidian_under_three_of_air_five_wide_each_way() {
        let mut world = World::flat(blocks::END_STONE, 50);
        world.with_context(1, |context| {
            assert!(EndPlatform.place(context, BlockPos::new(4, 49, 3)));
        });
        for dx in -3_i32..=3 {
            for dz in -3_i32..=3 {
                let inside = dx.abs() <= 2 && dz.abs() <= 2;
                let at = |y: i32| world.get(BlockPos::new(4 + dx, y, 3 + dz));
                if inside {
                    assert_eq!(at(48), blocks::OBSIDIAN);
                    assert_eq!(at(49), blocks::AIR);
                    assert_eq!(at(50), blocks::AIR);
                    assert_eq!(at(51), blocks::AIR);
                    assert_eq!(at(47), blocks::END_STONE);
                } else {
                    assert_eq!(at(48), blocks::END_STONE);
                    assert_eq!(at(50), blocks::END_STONE);
                }
            }
        }
        // Only what differed was set: the floor and the ground's top two layers.
        assert_eq!(world.writes.len(), 25 * 3);
    }

    #[test]
    fn the_data_places_the_platform_where_arrivals_stand() {
        let (_, placed) = clustine_worldgen_data::feature::placed("minecraft:end_platform")
            .expect("the data has the platform");
        let positions = placed
            .data
            .get("placement")
            .map(|placement| format!("{placement:?}"));
        assert!(
            positions.is_some_and(|text| text.contains("\"100\"") && text.contains("\"49\"")),
            "{placed:?}"
        );
        assert_eq!(END_PLATFORM, BlockPos::new(100, 49, 0));
        assert_eq!(platform_blocks(END_PLATFORM).count(), 100);
    }
}
