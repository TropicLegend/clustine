//! One feature run: the features of a chunk, placed as the game's
//! `ChunkGenerator.applyBiomeDecoration` places them, leaving out the structures'
//! pieces (phase S).

use std::collections::BTreeSet;

use clustine_noise::WorldgenRandom;
use clustine_world::generation::{Neighbourhood, Placing};
use clustine_world::{BlockPos, ChunkPos};
use clustine_worldgen_data::feature as data;

use crate::context::{Context, Level};
use crate::settings::{FeatureSettings, STEPS};

/// Places the features of `centre` into `view`.
///
/// The decoration seed comes from the world's seed and the chunk's lowest corner. The
/// features that can be placed are those the biomes anywhere in the three by three
/// chunks list; for each step, in the order of their places in the step's list, each
/// is seeded by its place and the step and placed from the corner, each in an attempt
/// of its own (ADR-0021), which the game's `try` around each feature stands for.
pub fn decorate(settings: &FeatureSettings, view: &mut dyn Neighbourhood, centre: ChunkPos) {
    let origin = BlockPos::new(centre.x * 16, settings.min_y(), centre.z * 16);
    // The game seeds its generator from the clock before it sets the decoration seed,
    // which replaces that seed; nothing of it remains.
    let mut random = WorldgenRandom::from_seed(0);
    let decoration = random.set_decoration_seed(settings.seed(), origin.x, origin.z);

    let mut biomes = BTreeSet::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            let (x, z) = ((centre.x + dx) * 16, (centre.z + dz) * 16);
            for y in (settings.min_y()..settings.min_y() + settings.height()).step_by(4) {
                for cell_x in 0..4 {
                    for cell_z in 0..4 {
                        let at = BlockPos::new(x + cell_x * 4, y, z + cell_z * 4);
                        biomes.insert(view.biome(at).0);
                    }
                }
            }
        }
    }

    // The region's own random source, which the game makes once for the run.
    let mut region_random = None;
    for step in 0..STEPS.max(settings.steps()) {
        let mut places = BTreeSet::new();
        for biome in &biomes {
            places.extend(settings.listed(*biome, step).iter().copied());
        }
        for place in places {
            let placed = settings.step(step)[place];
            random.set_feature_seed(decoration, place as i32, step as i32);
            let name = data::placed_features()[placed].name;
            view.attempt(Placing::Feature(name), &mut |view| {
                let mut level =
                    Level::new(view, settings.zoom(), settings.min_y(), settings.height());
                level.carry_region_random(region_random.take());
                let mut context = Context {
                    level: &mut level,
                    random: &mut random,
                    settings,
                    top: Some(placed),
                };
                settings.placed(placed).place(&mut context, origin);
                region_random = level.carry_region_random(None);
            });
        }
    }
}
