//! The land with features as the scheduler makes it (`FeatureStages`, step F1c of
//! `docs/groundwork/f-plan.md`): the runs of `clustine-features` in the order of
//! ADR-0021 over the preview's terrain. Whether the features are the game's is the
//! judge's question (`crates/clustine-features/tests/game_features.rs`); here, that
//! the stages are a function of the position.

use std::sync::Arc;

use clustine_structures::template_blocks::TemplateLibrary;
use clustine_world::ChunkPos;
use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::stages::Stages;
use clustine_worldgen::{FeatureStages, PreviewStages, TerrainGenerator};

const SEED: i64 = 13579;

#[test]
fn the_land_with_features_is_the_same_whichever_chunk_is_asked_first_and_on_any_number_of_threads()
{
    let terrain = Arc::new(TerrainGenerator::new(SEED));
    let (a, b) = (ChunkPos::new(0, 0), ChunkPos::new(1, -1));
    let made = |threads: usize, first: ChunkPos, second: ChunkPos| {
        let scheduler = Scheduler::new(
            FeatureStages::new(Arc::clone(&terrain)),
            Options::for_processors(threads),
        );
        let first_made = scheduler.finished(first).expect("the land is made");
        let second_made = scheduler.finished(second).expect("the land is made");
        assert!(
            scheduler.figures().runs > 0,
            "the land with features runs its features"
        );
        if first == a {
            (first_made, second_made)
        } else {
            (second_made, first_made)
        }
    };
    assert_eq!(made(1, a, b), made(4, b, a));
}

#[test]
fn the_land_with_features_names_the_steps_built_and_whether_structures_start() {
    let identity = |stages: &dyn Stages| stages.settings().identity;
    let with = identity(&FeatureStages::new(Arc::new(TerrainGenerator::new(SEED))));
    let without = identity(&FeatureStages::new(Arc::new(
        TerrainGenerator::without_structures(SEED),
    )));
    let preview = identity(&PreviewStages::new(Arc::new(TerrainGenerator::new(SEED))));
    // Made without the templates' blocks, which a server reads from its operator's
    // jar: a world made so says so.
    assert_eq!(
        with,
        format!(
            "terrain;features {};entities {};minecraft:overworld;seed {SEED};no templates",
            clustine_features::GENERATION,
            clustine_sim::generation::ENTITIES_GENERATION
        )
    );
    assert_eq!(
        without,
        format!(
            "terrain;features {};entities {};minecraft:overworld;seed {SEED};no structures;no templates",
            clustine_features::GENERATION,
            clustine_sim::generation::ENTITIES_GENERATION
        )
    );
    let library = TemplateLibrary::from_files(Vec::<(String, Vec<u8>)>::new()).unwrap();
    let with_templates = identity(&FeatureStages::with_templates(
        Arc::new(TerrainGenerator::new(SEED)),
        Arc::new(library),
    ));
    assert_eq!(
        with_templates,
        format!(
            "terrain;features {};entities {};minecraft:overworld;seed {SEED}",
            clustine_features::GENERATION,
            clustine_sim::generation::ENTITIES_GENERATION
        )
    );
    assert_ne!(with, preview);
}
