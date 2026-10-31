//! `void_start_platform`: the 33 by 33 floor of stone, cobblestone in its middle at
//! (8, 3 above the origin, 8), that the flat world of the void preset begins on.
//!
//! No dump holds `VoidStartPlatformFeature` (none of `/home/claude/reference/*-javap`),
//! so this follows SteelMC's port of it for orientation
//! (`steel-core/src/worldgen/feature/features/void_start_platform.rs`), which matches
//! what is known of the game: the platform's centre `PLATFORM_OFFSET` (8, 3, 8), its
//! chunk `PLATFORM_ORIGIN_CHUNK`, a reach of one chunk and of 16 blocks by the
//! chessboard distance, the blocks set with the flags 2. Only the biome `the_void`
//! places it, which no world of Clustine has yet; a comparison with the official
//! server is to settle it once one has.

use clustine_data::blocks;
use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};

/// The platform's middle from the chunk's origin: x and z absolute, y above the
/// origin's (`PLATFORM_OFFSET`).
const PLATFORM_OFFSET: (i32, i32, i32) = (8, 3, 8);
/// How far from the middle the platform reaches, in blocks and in chunks.
const PLATFORM_RADIUS: i32 = 16;
const PLATFORM_RADIUS_CHUNKS: i32 = 1;

/// The chessboard distance between two columns (`checkerboardDistance`).
fn chessboard(x0: i32, z0: i32, x1: i32, z1: i32) -> i32 {
    (x0 - x1).abs().max((z0 - z1).abs())
}

#[derive(Debug)]
pub struct VoidStartPlatform;

impl VoidStartPlatform {
    /// The kind has no settings.
    pub fn read(reader: &Reader) -> Read<Self> {
        reader.finish()?;
        Ok(Self)
    }
}

impl Feature for VoidStartPlatform {
    /// Nothing in a chunk more than one from the platform's; else, in the origin's
    /// chunk, z then x, each column within 16 of the middle gets stone at the
    /// middle's y, and the middle cobblestone.
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let chunk = origin.chunk();
        let (offset_x, offset_y, offset_z) = PLATFORM_OFFSET;
        if chessboard(chunk.x, chunk.z, offset_x >> 4, offset_z >> 4) > PLATFORM_RADIUS_CHUNKS {
            return true;
        }
        let middle = BlockPos::new(offset_x, origin.y + offset_y, offset_z);
        for z in chunk.z * 16..chunk.z * 16 + 16 {
            for x in chunk.x * 16..chunk.x * 16 + 16 {
                if chessboard(middle.x, middle.z, x, z) <= PLATFORM_RADIUS {
                    let at = BlockPos::new(x, middle.y, z);
                    let state = if at == middle {
                        blocks::COBBLESTONE
                    } else {
                        blocks::STONE
                    };
                    context.level.set_block(at, state);
                }
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
    fn the_void_s_platform_is_stone_round_cobblestone_in_its_chunk_s_part() {
        let mut world = World::flat(blocks::AIR, -100);
        world.with_context(1, |context| {
            assert!(VoidStartPlatform.place(context, BlockPos::new(0, -64, 0)));
        });
        assert_eq!(world.get(BlockPos::new(8, -61, 8)), blocks::COBBLESTONE);
        assert_eq!(world.get(BlockPos::new(0, -61, 15)), blocks::STONE);
        // The whole chunk is within 16 of the middle.
        assert_eq!(world.writes.len(), 256);
        // A chunk beside it gets the part within 16: x from -8 to -1.
        let mut world = World::flat(blocks::AIR, -100);
        world.with_context(1, |context| {
            assert!(VoidStartPlatform.place(context, BlockPos::new(-16, -64, 0)));
        });
        assert_eq!(world.writes.len(), 8 * 16);
        assert!(
            world
                .writes
                .iter()
                .all(|(at, state)| at.x >= -8 && at.y == -61 && *state == blocks::STONE)
        );
        // Two chunks off, nothing.
        let mut world = World::flat(blocks::AIR, -100);
        world.with_context(1, |context| {
            assert!(VoidStartPlatform.place(context, BlockPos::new(32, -64, 0)));
        });
        assert!(world.writes.is_empty());
    }
}
