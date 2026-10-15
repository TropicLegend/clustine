//! Where a new world's spawn is, as the game puts it (`MinecraftServer.setInitialSpawn`
//! of 26.3), from the chunk its search of the climate ends in
//! (`clustine_terrain::spawn`).
//!
//! From that chunk the game takes chunks in a square spiral, 11 by 11 of them at most,
//! and in each tries its columns from the north-west corner, x outer and z inner, for
//! one a player could stand on (`PlayerSpawnFinder.getSpawnPosInChunk`); the first it
//! finds is the spawn. Nearly always that is the corner column of the first chunk that
//! has a dry one. A column whose top is water or lava is refused, and so is one where a
//! fluid comes before the ground; the ground is the highest block whose collision
//! covers its top, and the spawn is the block above it.
//!
//! The game reads chunks with their features. The generator has none yet (phase F of
//! `docs/groundwork/terrain-plan.md`), so a tree on the column changes the spawn's
//! height here (the game's player stands on its canopy), and ice, a lake or leaves
//! over water on a column tried earlier can change which column it is
//! (`docs/groundwork/t8-plan.md`, section 2).

use clustine_data::Face;
use clustine_data::faces::face_full;
use clustine_world::{BlockPos, Chunk, ChunkPos};

/// How many chunks the spiral takes: 11 by 11.
pub const SPIRAL_CHUNKS: usize = 121;

/// The height of the spawn where no chunk of the spiral has a column to stand on
/// (`ChunkGenerator.getSpawnHeight`, which the game's generators of the overworld
/// keep).
pub const FALLBACK_HEIGHT: i32 = 64;

/// The offsets from the first chunk of the spiral, in the order the game takes them:
/// (0, 0), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1),
/// (2, -1), and on.
pub fn spiral() -> impl Iterator<Item = (i32, i32)> {
    let (mut x, mut z, mut step_x, mut step_z) = (0, 0, 0, -1);
    (0..SPIRAL_CHUNKS).map(move |_| {
        let here = (x, z);
        // The game turns at the corners of the square it is walking round.
        if x == z || (x < 0 && x == -z) || (x > 0 && x == 1 - z) {
            (step_x, step_z) = (-step_z, step_x);
        }
        x += step_x;
        z += step_z;
        here
    })
}

/// The spawn of a world whose search of the climate ended in the chunk `origin`, with
/// `chunk` giving a chunk of the world as it is made. Asks for one chunk after another
/// until one has a column to stand on, which is mostly the first.
pub fn place_to_stand<C: std::ops::Deref<Target = Chunk>>(
    origin: ChunkPos,
    mut chunk: impl FnMut(ChunkPos) -> C,
) -> BlockPos {
    for (x, z) in spiral() {
        let position = ChunkPos::new(origin.x + x, origin.z + z);
        if let Some(spawn) = in_chunk(&chunk(position), position) {
            return spawn;
        }
    }
    BlockPos::new(origin.x * 16 + 8, FALLBACK_HEIGHT, origin.z * 16 + 8)
}

/// The first column of the chunk, x outer and z inner, that a player could stand on,
/// as the block their feet are in.
pub fn in_chunk(chunk: &Chunk, position: ChunkPos) -> Option<BlockPos> {
    (0..16).find_map(|x| {
        (0..16).find_map(|z| {
            let y = feet(chunk, x, z)?;
            Some(BlockPos::new(
                position.x * 16 + x as i32,
                y,
                position.z * 16 + z as i32,
            ))
        })
    })
}

/// Where a player's feet would be on the column `x`, `z` of the chunk, or `None` where
/// they could not stand on it (`PlayerSpawnFinder.getLevelRespawnPos`, in a dimension
/// without a ceiling).
pub fn feet(chunk: &Chunk, x: usize, z: usize) -> Option<i32> {
    let heights = chunk.heights();
    let column = z << 4 | x;
    // What the game's heightmaps give: the height above the highest block counted, the
    // bottom of the world where there is none.
    let height = |map: &[u16]| chunk.min_y() + i32::from(map[column]);
    let motion_blocking = height(&heights.motion_blocking);
    let world_surface = height(&heights.world_surface);
    // The top is a fluid: the surface is no higher than what blocks motion or holds a
    // fluid, and above what blocks motion alone.
    if world_surface <= motion_blocking && world_surface > height(&heights.ocean_floor) {
        return None;
    }
    // Down from above the top; the first fluid refuses the column.
    for y in (chunk.min_y()..=motion_blocking + 1).rev() {
        let Some(state) = chunk.get(x, y, z) else {
            continue;
        };
        if state.has_fluid() {
            return None;
        }
        let collision: Vec<[f64; 6]> = state.props().collision_shape().boxes().collect();
        if face_full(&collision, Face::Up) {
            return Some(y + 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spiral_walks_round_its_first_chunk_as_the_game_does() {
        let first: Vec<(i32, i32)> = spiral().take(10).collect();
        assert_eq!(
            first,
            [
                (0, 0),
                (1, 0),
                (1, 1),
                (0, 1),
                (-1, 1),
                (-1, 0),
                (-1, -1),
                (0, -1),
                (1, -1),
                (2, -1)
            ]
        );
    }

    #[test]
    fn the_spiral_takes_every_chunk_of_eleven_by_eleven_once() {
        let mut all: Vec<(i32, i32)> = spiral().collect();
        assert_eq!(all.len(), SPIRAL_CHUNKS);
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), SPIRAL_CHUNKS);
        assert!(all.iter().all(|(x, z)| x.abs() <= 5 && z.abs() <= 5));
    }
}
