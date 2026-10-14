//! The preview of the terrain is a function of the seed and the position, and what it
//! makes is what `clustine-terrain` and `clustine-light` say, put together.

use clustine_data::{DIMENSION_TYPES, DimensionType};
use clustine_light::Around;
use clustine_terrain::biome::zoom::obfuscate_seed;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::Terrain;
use clustine_terrain::surface::Biomes;
use clustine_world::{Biome, Chunk, ChunkGenerator, ChunkPos, SECTION_SIZE};
use clustine_worldgen::{FlatGenerator, TERRAIN_VERSION, TerrainGenerator};

const SEED: i64 = 13579;

fn overworld() -> &'static DimensionType {
    DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .unwrap()
}

/// The chunk's blocks straight from the terrain, with one biome and nothing else.
fn carved(terrain: &Terrain, source: &BiomeSource, position: ChunkPos) -> Chunk {
    let biomes = Biomes::from_source(source, position.x, position.z);
    let carved = terrain.carved(position.x, position.z, &biomes, source);
    carved.chunk.into_chunk(overworld(), Biome(0))
}

#[test]
fn a_chunk_is_the_same_whatever_was_asked_before_and_whatever_is_kept() {
    // Neighbours, which share what is kept, and one far off.
    let positions = [
        ChunkPos::new(-1, 0),
        ChunkPos::new(0, 0),
        ChunkPos::new(0, 1),
        ChunkPos::new(30, -17),
    ];

    // In this order, each beside what the ones before it left.
    let warm = TerrainGenerator::new(SEED);
    let forwards: Vec<Chunk> = positions.iter().map(|at| warm.generate(*at)).collect();
    // A second time, with everything kept.
    for (position, chunk) in positions.iter().zip(&forwards) {
        assert_eq!(warm.generate(*position), *chunk, "{position:?} again");
    }

    // By another generator of the same seed, in the opposite order, with nothing
    // kept before each.
    let cold = TerrainGenerator::new(SEED);
    for (position, chunk) in positions.iter().zip(&forwards).rev() {
        cold.forget();
        assert_eq!(cold.kept(), 0);
        assert_eq!(cold.generate(*position), *chunk, "{position:?} cold");
        // The chunk and the eight around it, and no more.
        assert_eq!(cold.kept(), 9);
    }

    for chunk in &forwards {
        assert_eq!(chunk.generated(), TERRAIN_VERSION);
        assert!(!chunk.with_failure());
        assert!(chunk.light().is_some());
    }
    // Another seed is another world.
    let other = TerrainGenerator::new(SEED + 1);
    assert_ne!(other.generate(positions[1]), forwards[1]);
}

#[test]
fn a_chunk_has_the_blocks_of_the_carved_terrain_and_the_biomes_of_the_source() {
    let terrain = Terrain::new(SEED, Dimension::Overworld);
    let source = BiomeSource::new(SEED, Dimension::Overworld);
    let generator = TerrainGenerator::new(SEED);
    let mut states = std::collections::BTreeSet::new();
    let mut biomes = std::collections::BTreeSet::new();
    for position in [ChunkPos::new(0, 0), ChunkPos::new(-21, 9)] {
        let chunk = generator.generate(position);
        let around = Biomes::from_source(&source, position.x, position.z);
        let expected = terrain
            .carved(position.x, position.z, &around, &source)
            .chunk;

        assert_eq!(chunk.min_y(), overworld().min_y);
        assert_eq!(chunk.height(), overworld().height as usize);
        for y in chunk.min_y()..chunk.min_y() + chunk.height() as i32 {
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let state = chunk.get(x, y, z).unwrap();
                    assert_eq!(state, expected.get(x, y, z), "{position:?} {x} {y} {z}");
                    states.insert(state);
                }
            }
        }
        // The marks, each once, as a chunk keeps them.
        let mut marks = expected.marks.clone();
        marks.dedup();
        assert_eq!(chunk.marks(), marks);

        let cells = source.chunk(position.x, position.z);
        assert_eq!(cells.len(), chunk.sections().len());
        for (section, cells) in chunk.sections().iter().zip(&cells) {
            assert!(section.biomes().eq(cells.iter().copied()), "{position:?}");
            biomes.extend(cells.iter().copied());
        }
    }
    // Land, and not one block throughout: stone, water, air and more.
    assert!(states.len() > 5, "{states:?}");
    assert!(!biomes.is_empty());
}

#[test]
fn a_chunk_carries_the_light_of_its_blocks_beside_its_eight_neighbours() {
    let terrain = Terrain::new(SEED, Dimension::Overworld);
    let source = BiomeSource::new(SEED, Dimension::Overworld);
    let generator = TerrainGenerator::new(SEED);
    let position = ChunkPos::new(4, -3);
    let chunk = generator.generate(position);

    // The nine, each made alone by the terrain; north-west first, row by row.
    let at = |x: i32, z: i32| {
        carved(
            &terrain,
            &source,
            ChunkPos::new(position.x + x, position.z + z),
        )
    };
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
        sky: true,
    });
    // The rules do not say whose chunks they lit; the generator does.
    assert_eq!(expected.beside, 0);
    expected.beside = TERRAIN_VERSION;
    let light = chunk.light().expect("a generated chunk has light");
    assert_eq!(light.made_by, clustine_light::VERSION);
    assert_eq!(*light, expected);

    // The neighbours count: alone, the chunk would be lit otherwise at its sides
    // wherever the land beside it is not the same.
    let alone = clustine_light::light(&Around {
        centre: &centre,
        neighbours: [None; 8],
        sky: true,
    });
    assert_ne!(alone.sky, expected.sky);
}

#[test]
fn the_settings_name_the_seed_and_the_version() {
    let settings = TerrainGenerator::new(SEED).settings();
    assert_eq!(
        settings,
        format!("terrain;preview {TERRAIN_VERSION};minecraft:overworld;seed 13579")
    );
    assert_ne!(settings, TerrainGenerator::new(-SEED).settings());
    assert_ne!(settings, FlatGenerator::classic().settings());
    assert!(
        TerrainGenerator::new(-SEED)
            .settings()
            .ends_with("seed -13579")
    );
}

#[test]
fn a_client_is_told_the_hash_of_the_seed_and_where_the_sea_is() {
    let generator = TerrainGenerator::new(SEED);
    assert_eq!(generator.seed(), SEED);
    assert_eq!(generator.hashed_seed(), obfuscate_seed(SEED));
    assert_ne!(generator.hashed_seed(), 0);
    assert_eq!(generator.sea_level(), 63);
}

/// Where players enter: above the middle of the block at the origin, their feet in
/// the first block over the ground or the water that is neither.
#[test]
fn players_enter_on_the_ground_of_the_column_at_the_origin_or_on_its_water() {
    // The first has sea at the origin; the others were not chosen.
    let mut on_water = 0;
    for seed in [SEED, 1, 2, 3, -4] {
        let generator = TerrainGenerator::new(seed);
        let spawn = generator.spawn_point();
        assert_eq!((spawn.x, spawn.z), (0.5, 0.5));
        assert_eq!(spawn.y.fract(), 0.0);
        let feet = spawn.y as i32;

        let chunk = generator.generate(ChunkPos::new(0, 0));
        let state = |y: i32| chunk.get(0, y, 0).unwrap();
        let below = state(feet - 1);
        assert!(!below.is_air(), "seed {seed}: on air at {feet}");
        assert!(
            below.props().in_motion_heightmap() || below.has_fluid(),
            "seed {seed}: on {below:?}"
        );
        on_water += usize::from(below.has_fluid());
        // Nothing above blocks motion or is a fluid: the player is not in the
        // ground, not under an overhang's rock and not under water.
        for y in feet..chunk.min_y() + chunk.height() as i32 {
            let above = state(y);
            assert!(
                !above.props().in_motion_heightmap() && !above.has_fluid(),
                "seed {seed}: {above:?} at {y} over the feet at {feet}"
            );
        }
    }
    assert!(on_water >= 1, "the sea of seed {SEED} is at its origin");

    // The sea of that seed is 63 high, and the player's feet are over it.
    assert_eq!(TerrainGenerator::new(SEED).spawn_point().y, 63.0);
}
