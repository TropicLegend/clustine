//! The End served with its features (step P10a of `docs/groundwork/p-plan.md`, section
//! 19.2): the spikes round the arena with their crystals made as entities of their
//! chunks, the platform at (100, 49, 0), and chorus in the highlands. Whether the
//! features are the game's block for block is the judge's question
//! (`tests/game_features.rs`, its End's tests); here, that the land has them.

use std::sync::Arc;

use clustine_data::{BlockState, blocks, entity_types};
use clustine_features::features::end::end_spike::spikes_of_seed;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_world::{BlockPos, Chunk, ChunkPos, EndCrystal, EntityData, SECTION_SIZE};
use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::stages::Stages;
use clustine_worldgen::{FeatureStages, TerrainGenerator};

const SEED: i64 = 13579;

fn the_end() -> Scheduler<FeatureStages> {
    let terrain = Arc::new(TerrainGenerator::new(SEED, Dimension::End));
    let stages = FeatureStages::new(terrain);
    assert!(
        stages
            .settings()
            .identity
            .contains(";minecraft:the_end;seed 13579")
    );
    Scheduler::new(stages, Options::for_processors(4))
}

/// The block at `at` of `chunk`, which holds it.
fn block(chunk: &Chunk, at: BlockPos) -> BlockState {
    chunk
        .get(
            at.x.rem_euclid(16) as usize,
            at.y,
            at.z.rem_euclid(16) as usize,
        )
        .expect("within the End's height")
}

#[test]
fn the_end_s_spikes_stand_with_their_crystals_as_entities_of_their_chunks() {
    let scheduler = the_end();
    let spikes = spikes_of_seed(SEED);
    for spike in spikes {
        let (x, h, z) = (spike.centre_x, spike.height, spike.centre_z);
        let at = BlockPos::new(x, h, z).chunk();
        let chunk = scheduler.finished(at).expect("the land is made");
        assert!(!chunk.with_failure(), "{at:?}");
        assert_eq!(block(&chunk, BlockPos::new(x, h - 1, z)), blocks::OBSIDIAN);
        assert_eq!(block(&chunk, BlockPos::new(x, 40, z)), blocks::OBSIDIAN);
        assert_eq!(block(&chunk, BlockPos::new(x, h, z)), blocks::BEDROCK);
        assert_eq!(block(&chunk, BlockPos::new(x, h + 1, z)), blocks::FIRE);
        assert_eq!(block(&chunk, BlockPos::new(x, h + 2, z)), blocks::AIR);
        let crystals: Vec<_> = chunk
            .entities()
            .iter()
            .filter(|entity| i32::from(entity.kind) == entity_types::END_CRYSTAL)
            .collect();
        assert_eq!(crystals.len(), 1, "{at:?}");
        let crystal = crystals[0];
        assert_eq!(crystal.position, spike.crystal_position());
        assert_eq!(
            crystal.data,
            EntityData::EndCrystal(EndCrystal {
                beam_target: None,
                show_bottom: true,
            })
        );
        // A caged spike's bars stand on its top's rim and roof it.
        let rim = scheduler
            .finished(BlockPos::new(x + 2, h, z).chunk())
            .expect("the land is made");
        let bars = block(&rim, BlockPos::new(x + 2, h, z));
        assert_eq!(
            bars.block_id() == blocks::IRON_BARS.block_id(),
            spike.guarded,
            "{spike:?}"
        );
    }
    // Two of the ten are caged.
    assert_eq!(spikes.iter().filter(|spike| spike.guarded).count(), 2);
    assert_eq!(scheduler.figures().failed_placements, 0);
}

#[test]
fn the_end_s_platform_is_made_with_its_chunks_at_100_49_0() {
    let scheduler = the_end();
    for z in -2..=2 {
        let chunk = scheduler
            .finished(BlockPos::new(100, 48, z).chunk())
            .expect("the land is made");
        for x in 98..=102 {
            assert_eq!(block(&chunk, BlockPos::new(x, 48, z)), blocks::OBSIDIAN);
            for y in 49..=51 {
                assert_eq!(block(&chunk, BlockPos::new(x, y, z)), blocks::AIR);
            }
        }
    }
}

/// The first chunk out from the origin, beyond the middle island, whose middle at y 64
/// is of `biome`.
fn chunk_of(biome: &str) -> ChunkPos {
    let names = clustine_data::synced_registry("minecraft:worldgen/biome")
        .expect("the biomes are a synchronised registry")
        .entries;
    let source = BiomeSource::new(SEED, Dimension::End);
    for radius in 70i32..200 {
        for x in -radius..=radius {
            for z in -radius..=radius {
                if x.abs() != radius && z.abs() != radius {
                    continue;
                }
                let found = source.found(x * 4 + 2, 16, z * 4 + 2);
                if names[usize::from(found.biome.0)] == biome {
                    return ChunkPos::new(x, z);
                }
            }
        }
    }
    panic!("no {biome} out from the origin");
}

#[test]
fn chorus_grows_in_the_end_s_highlands() {
    let scheduler = the_end();
    let highlands = chunk_of("minecraft:end_highlands");
    let mut plants = 0;
    let mut flowers = 0;
    for dx in -1..=1 {
        for dz in -1..=1 {
            let at = ChunkPos::new(highlands.x + dx, highlands.z + dz);
            let chunk = scheduler.finished(at).expect("the land is made");
            assert!(!chunk.with_failure(), "{at:?}");
            for y in chunk.min_y()..chunk.min_y() + chunk.height() as i32 {
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let state = chunk.get(x, y, z).expect("within the height");
                        if state.block_id() == blocks::CHORUS_PLANT.block_id() {
                            plants += 1;
                        } else if state.block_id() == blocks::CHORUS_FLOWER.block_id() {
                            flowers += 1;
                            assert_eq!(state.property("age"), Some("5"));
                        }
                    }
                }
            }
        }
    }
    assert!(plants > 0 && flowers > 0, "no chorus about {highlands:?}");
    assert_eq!(scheduler.figures().failed_placements, 0);
}
