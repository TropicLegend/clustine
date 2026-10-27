//! The game seed regions are given, tested from the plan (`docs/groundwork/n9-plan.md`,
//! decision 3 of section 18 and section 14) and the lead's decision on it by someone who
//! did not write it: a generated world's regions get its seed, which slime chunks and
//! the zoomed biome read; a flat world's get 0 until the world's identity keeps a seed
//! of its own (the plan's identity-kept seed, which is not built).
//!
//! The way is the store's: the world's identity (the generator's settings) gives the
//! seed it opens regions with (`Restored::seed`, `clustine_worldstore::world_seed`), and
//! the worker makes the region's configuration of it (`RegionSettings::with_spawn`).

use std::sync::Arc;

use clustine_sim::api::Inventory;
use clustine_worker::RegionSettings;
use clustine_world::{BlockPos, ChunkGenerator};
use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::{FeatureStages, FlatGenerator, TerrainGenerator};
use clustine_worldstore::world_seed;

fn settings() -> RegionSettings {
    RegionSettings {
        starting_inventory: Inventory::default(),
        return_after: 0,
        lowest_y: -64,
        rings: true,
    }
}

/// The game seed of the regions of the world `generator` makes.
fn regions_game_seed(generator: &dyn ChunkGenerator) -> i64 {
    let seed = world_seed(&generator.settings().identity);
    settings()
        .with_spawn(BlockPos::new(0, 64, 0), 17, seed)
        .game_seed
}

/// Decision 3 of section 18: a generated world's regions get its seed, whatever it is.
#[test]
fn a_generated_world_s_regions_get_its_seed() {
    for seed in [3, -1_234_567_890_123, i64::MAX] {
        let terrain = Arc::new(TerrainGenerator::new(seed));
        let world = Scheduler::new(FeatureStages::new(terrain), Options::for_processors(1));
        assert_eq!(regions_game_seed(&world), seed, "the world of seed {seed}");
    }
}

/// The lead's decision (N9h, "Give regions the world's game seed and the game's zoomed
/// biome"): a flat world's regions get 0, which an official server given `level-seed=0`
/// shares, until its identity keeps a seed of its own.
#[test]
fn a_flat_world_s_regions_get_nought() {
    assert_eq!(regions_game_seed(&FlatGenerator::classic()), 0);
}
