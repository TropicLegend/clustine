//! Where the game begins to look for a new world's spawn: the place nearest the origin
//! whose climate suits a start, as its noise settings' spawn target says
//! (`NoiseSpawnFinder`, which `ChunkGenerator.getOrigin` asks when a world is made).
//!
//! The search is two passes of rings, each around the best place so far: one every
//! 512 blocks out to 2,048, then one every 32 blocks out to 512. A place's fitness is
//! how far its climate lies outside the target, scaled by 2048², plus its own
//! squared distance from the origin, so that of two places with the same climate the
//! nearer wins. Where on a ring the game samples rests on HotSpot's sine and cosine
//! in their last bit, so those offsets are emitted from the game's own arithmetic
//! ([`spawn_search::PASSES`]) rather than worked out here. The block the search gives
//! is where the spiral for a column to stand on begins; that is not this crate's.

use clustine_worldgen_data::{NoiseRouter, OverworldRouter, spawn_search};

use crate::biome::climate::quantise;

/// The block x and z of the overworld of this seed that the game searches its spawn
/// from.
pub fn search_origin(seed: i64) -> [i32; 2] {
    search_origin_of(&OverworldRouter::new(seed))
}

/// The same for a world whose terrain is this router's. Where the dimension's settings
/// name no spawn target, the game takes the origin.
pub fn search_origin_of<R: NoiseRouter>(router: &R) -> [i32; 2] {
    if router.settings().spawn_target.is_empty() {
        return [0, 0];
    }
    let mut memory = R::Memory::default();
    let mut best = ([0, 0], fitness(router, &mut memory, 0, 0));
    for pass in &spawn_search::PASSES {
        // The ring's centre is the best place when the pass begins, not as it goes.
        let [centre_x, centre_z] = best.0;
        for [x, z] in pass.offsets {
            let place = [centre_x + x, centre_z + z];
            let fitness = fitness(router, &mut memory, place[0], place[1]);
            // Only a strictly fitter place replaces the best, so a tie keeps the
            // earlier.
            if fitness < best.1 {
                best = (place, fitness);
            }
        }
    }
    best.0
}

/// How badly the block x, z suits a spawn; lower is better. The climate is sampled at
/// the corner of the block's biome cell, at height 0, and the distance of the block
/// itself is added as it is.
pub fn fitness<R: NoiseRouter>(router: &R, memory: &mut R::Memory, x: i32, z: i32) -> i64 {
    let (cell_x, cell_z) = (x & !3, z & !3);
    let climate = router
        .settings()
        .spawn_target
        .iter()
        .map(|target| {
            target
                .iter()
                .map(|range| {
                    let value = router
                        .named(memory, range.function, cell_x, 0, cell_z)
                        .expect("the emitter refuses a spawn target the router does not reach");
                    let distance =
                        outside(quantise(range.min), quantise(range.max), quantise(value));
                    distance * distance
                })
                .sum::<i64>()
        })
        .min()
        .unwrap_or(i64::MAX);
    let (x, z) = (i64::from(x), i64::from(z));
    climate * 2048 * 2048 + x * x + z * z
}

/// How far a value lies outside a range, zero inside it (`Climate.Parameter.distance`).
fn outside(min: i64, max: i64, value: i64) -> i64 {
    let above = value - max;
    if above > 0 {
        above
    } else {
        (min - value).max(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_worldgen_data::{NetherRouter, noise_settings};

    #[test]
    fn a_value_inside_its_range_is_no_distance_and_outside_it_the_distance_to_the_nearer_end() {
        assert_eq!(outside(-1100, 10000, 0), 0);
        assert_eq!(outside(-1100, 10000, -1100), 0);
        assert_eq!(outside(-1100, 10000, 10000), 0);
        assert_eq!(outside(-1100, 10000, -1500), 400);
        assert_eq!(outside(-1100, 10000, 10007), 7);
    }

    #[test]
    fn the_passes_are_the_games_two_with_the_number_of_samples_the_plan_counted() {
        let passes: Vec<(u32, usize)> = spawn_search::PASSES
            .iter()
            .map(|pass| (pass.increment, pass.offsets.len()))
            .collect();
        assert_eq!(passes, [(512, 65), (32, 862)]);
        // Each pass begins on the ring of its increment, straight along z.
        assert_eq!(spawn_search::PASSES[0].offsets[0], [0, 512]);
        assert_eq!(spawn_search::PASSES[1].offsets[0], [0, 32]);
    }

    #[test]
    fn a_world_without_a_spawn_target_searches_from_the_origin() {
        assert!(noise_settings::NETHER.spawn_target.is_empty());
        assert_eq!(search_origin_of(&NetherRouter::new(13579)), [0, 0]);
    }

    #[test]
    fn the_fitness_is_the_climates_distance_scaled_plus_the_squared_distance_of_the_block() {
        let router = OverworldRouter::new(13579);
        let mut memory = Default::default();
        // Within a biome cell the climate is the same, so the fitness differs by the
        // block's own distance alone.
        let corner = fitness(&router, &mut memory, 1024, -512);
        let inside = fitness(&router, &mut memory, 1027, -509);
        assert_eq!(
            inside - corner,
            (1027 * 1027 + 509 * 509) - (1024 * 1024 + 512 * 512)
        );
        assert_eq!((corner - (1024 * 1024 + 512 * 512)) % (2048 * 2048), 0);
    }
}
