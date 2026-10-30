//! The Nether served (step P5 of `docs/groundwork/p-plan.md`): the terrain crate's
//! Nether in the dimension's 256 blocks, lit without a sky, told its own sea, and
//! decorated with the Nether's own features. Whether the features are the game's is
//! the judge's question (`tests/game_features.rs` with
//! `CLUSTINE_GAME_FEATURES_DIMENSION=the_nether`); here, that the land is the Nether's.

use std::sync::Arc;

use clustine_data::{BlockState, blocks};
use clustine_light::Around;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::Terrain;
use clustine_terrain::surface::Biomes;
use clustine_world::SectionLight;
use clustine_world::{Chunk, ChunkGenerator, ChunkPos, SECTION_SIZE};
use clustine_worldgen::features::makes_mobs;
use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::stages::Stages;
use clustine_worldgen::{FeatureStages, TERRAIN_VERSION, TerrainGenerator};

const SEED: i64 = 13579;

fn blocks_of(chunk: &Chunk) -> impl Iterator<Item = (usize, i32, usize, BlockState)> + '_ {
    (chunk.min_y()..chunk.min_y() + chunk.height() as i32).flat_map(move |y| {
        (0..SECTION_SIZE).flat_map(move |z| {
            (0..SECTION_SIZE).map(move |x| (x, y, z, chunk.get(x, y, z).unwrap()))
        })
    })
}

#[test]
fn a_nether_chunk_is_the_terrain_s_nether_in_the_dimension_s_256_blocks() {
    let generator = TerrainGenerator::without_structures_in(SEED, Dimension::Nether);
    let terrain = Terrain::new(SEED, Dimension::Nether);
    let source = BiomeSource::new(SEED, Dimension::Nether);
    let kind = Dimension::Nether.rules();
    let mut lava = 0;
    for position in [ChunkPos::new(0, 0), ChunkPos::new(-9, 23)] {
        let chunk = generator.generate(position);
        assert_eq!((chunk.min_y(), chunk.height()), (0, 256));
        let around = Biomes::from_source(&source, position.x, position.z);
        let expected = terrain
            .carved(position.x, position.z, &around, &source)
            .chunk
            .into_chunk(kind, clustine_world::Biome(0));
        for (x, y, z, state) in blocks_of(&chunk) {
            assert_eq!(
                Some(state),
                expected.get(x, y, z),
                "{position:?} {x} {y} {z}"
            );
            if state == blocks::LAVA {
                // The Nether's sea is of lava, up to its sea level and not above.
                assert!(y < TerrainGenerator::sea_level_of(Dimension::Nether), "{y}");
                lava += 1;
            }
        }
        // A floor and a roof of bedrock, and nothing above the roof the generator
        // makes: the dimension is 256 high, its generator 128.
        assert!((0..16).all(|x| chunk.get(x, 0, 0) == Some(blocks::BEDROCK)));
        assert!((0..16).all(|x| chunk.get(x, 127, 0) == Some(blocks::BEDROCK)));
        assert!(blocks_of(&chunk).all(|(_, y, _, state)| y < 128 || state == blocks::AIR));
        // The biomes are the Nether's source's.
        let cells = source.chunk(position.x, position.z);
        assert_eq!(cells.len(), chunk.sections().len());
        for (section, cells) in chunk.sections().iter().zip(&cells) {
            assert!(section.biomes().eq(cells.iter().copied()), "{position:?}");
        }
    }
    assert!(lava > 0, "no sea of lava");
}

#[test]
fn a_nether_chunk_is_lit_without_a_sky() {
    let generator = TerrainGenerator::new(SEED, Dimension::Nether);
    let position = ChunkPos::new(3, -5);
    let chunk = generator.generate(position);
    assert_eq!(chunk.generated(), TERRAIN_VERSION);
    let at = |x: i32, z: i32| generator.unlit(ChunkPos::new(position.x + x, position.z + z));
    let centre = at(0, 0);
    let beside = [
        at(-1, -1),
        at(0, -1),
        at(1, -1),
        at(-1, 0),
        at(1, 0),
        at(-1, 1),
        at(0, 1),
        at(1, 1),
    ];
    let mut expected = clustine_light::light(&Around {
        centre: &centre,
        neighbours: beside.each_ref().map(Some),
        sky: false,
    });
    expected.beside = TERRAIN_VERSION;
    let light = chunk.light().expect("a generated chunk has light");
    assert_eq!(*light, expected);
    // No sky light anywhere, under the roof or above it; the lava gives light of its
    // own.
    assert!(
        light
            .sky
            .iter()
            .all(|section| *section == SectionLight::Absent)
    );
    assert!(
        light
            .block
            .iter()
            .any(|section| *section != SectionLight::Absent && *section != SectionLight::Dark)
    );
}

#[test]
fn the_nether_is_named_has_its_own_sea_and_its_origin_at_chunk_0_0() {
    let generator = TerrainGenerator::new(SEED, Dimension::Nether);
    assert_eq!(generator.level(), Dimension::Nether);
    assert_eq!(generator.dimension().name, "minecraft:the_nether");
    assert!(!generator.dimension().has_skylight);
    assert_eq!(
        generator.settings().identity,
        "terrain;preview;minecraft:the_nether;seed 13579"
    );
    assert_eq!(TerrainGenerator::sea_level_of(Dimension::Nether), 32);
    assert_eq!(TerrainGenerator::sea_level_of(Dimension::End), 0);
    // A client is still told the overworld's.
    assert_eq!(TerrainGenerator::sea_level(), 63);
    assert_eq!(generator.origin(), ChunkPos::new(0, 0));
    // The Nether makes mobs with its chunks; the End does not.
    assert!(makes_mobs(Dimension::Nether));
    assert!(!makes_mobs(Dimension::End));
}

/// The first chunk out from the origin whose middle at y 64 is of `biome`.
fn chunk_of(biome: &str) -> ChunkPos {
    let names = clustine_data::synced_registry("minecraft:worldgen/biome")
        .expect("the biomes are a synchronised registry")
        .entries;
    let source = BiomeSource::new(SEED, Dimension::Nether);
    for radius in 0i32..64 {
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
    panic!("no {biome} near the origin");
}

#[test]
fn the_nether_with_features_has_the_nether_s_own_features() {
    let terrain = Arc::new(TerrainGenerator::new(SEED, Dimension::Nether));
    let stages = FeatureStages::new(Arc::clone(&terrain));
    assert!(
        stages
            .settings()
            .identity
            .contains(";minecraft:the_nether;seed 13579")
    );
    assert_eq!(stages.dimension().name, "minecraft:the_nether");
    let scheduler = Scheduler::new(stages, Options::for_processors(4));
    // Fungi stand in the crimson forest, and only features make their stems.
    let crimson = chunk_of("minecraft:crimson_forest");
    let stems = [crimson, ChunkPos::new(crimson.x + 1, crimson.z)]
        .iter()
        .map(|at| {
            let chunk = scheduler.finished(*at).expect("the land is made");
            assert!(!chunk.with_failure(), "{at:?}");
            blocks_of(&chunk)
                .filter(|(.., state)| *state == blocks::CRIMSON_STEM)
                .count()
        })
        .sum::<usize>();
    assert!(stems > 0, "no fungus near {crimson:?}");
    // Glowstone hangs from the roofs here and there all over the Nether.
    let glowstone: usize = (-3..3)
        .flat_map(|x| (-3..3).map(move |z| ChunkPos::new(x, z)))
        .map(|at| {
            let chunk = scheduler.finished(at).expect("the land is made");
            blocks_of(&chunk)
                .filter(|(.., state)| *state == blocks::GLOWSTONE)
                .count()
        })
        .sum();
    assert!(glowstone > 0);
    let figures = scheduler.figures();
    assert_eq!(figures.failed_placements, 0);
}
