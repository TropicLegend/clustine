//! The preview of the terrain is a function of the seed and the position, and what it
//! makes is what `clustine-terrain` and `clustine-light` say, put together.

use clustine_data::{DIMENSION_TYPES, DimensionType};
use clustine_light::Around;
use clustine_structures::Structures;
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

/// A chunk of the seed whose ground a structure bends, 943 blocks of it: near the
/// origin, among the chunks the game's classes were asked for when the bending was
/// judged.
const BENT: ChunkPos = ChunkPos::new(-14, 15);

#[test]
fn a_chunk_has_the_blocks_of_the_terrain_bent_by_structures_and_the_biomes_of_the_source() {
    let structures = Structures::new(SEED, Dimension::Overworld);
    let (terrain, source) = (structures.terrain(), structures.biomes());
    let generator = TerrainGenerator::new(SEED);
    let mut states = std::collections::BTreeSet::new();
    let mut biomes = std::collections::BTreeSet::new();
    let mut bent = 0;
    for position in [ChunkPos::new(0, 0), ChunkPos::new(-21, 9), BENT] {
        let chunk = generator.generate(position);
        let around = Biomes::from_source(source, position.x, position.z);
        let starts_of = |x, z| structures.starts(x, z);
        let beard = Structures::beardifier(position.x, position.z, starts_of);
        let expected = terrain
            .carved_bent(position.x, position.z, &beard, &around, source)
            .chunk;
        let unbent = terrain
            .carved(position.x, position.z, &around, source)
            .chunk;
        let differs = |(x, y, z)| expected.get(x, y, z) != unbent.get(x, y, z);
        let blocks = (chunk.min_y()..chunk.min_y() + chunk.height() as i32).flat_map(|y| {
            (0..SECTION_SIZE).flat_map(move |z| (0..SECTION_SIZE).map(move |x| (x, y, z)))
        });
        bent += blocks.filter(|block| differs(*block)).count();

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
    // The village's chunk is not as the land would be without it.
    assert!(bent > 0);
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
fn the_settings_name_the_seed_and_the_version_apart() {
    let settings = TerrainGenerator::new(SEED).settings();
    assert_eq!(
        settings.identity,
        "terrain;preview;minecraft:overworld;seed 13579"
    );
    assert_eq!(settings.version, TERRAIN_VERSION);
    assert_ne!(settings, TerrainGenerator::new(-SEED).settings());
    assert_ne!(settings, FlatGenerator::classic().settings());
    assert!(
        TerrainGenerator::new(-SEED)
            .settings()
            .identity
            .ends_with("seed -13579")
    );
}

#[test]
fn a_client_is_told_the_hash_of_the_seed_and_where_the_sea_is() {
    let generator = TerrainGenerator::new(SEED);
    assert_eq!(generator.seed(), SEED);
    // Without a generator: an edge makes none.
    assert_eq!(TerrainGenerator::hashed_seed(SEED), obfuscate_seed(SEED));
    assert_ne!(TerrainGenerator::hashed_seed(SEED), 0);
    assert_eq!(TerrainGenerator::sea_level(), 63);
}

/// A new world's spawn is a place to stand: the block under the feet covers its top
/// and holds no fluid, and the feet are in no fluid. Where the game puts it, column for
/// column, is judged against the official server in `tests/official_spawn.rs`.
#[test]
fn a_new_worlds_spawn_is_dry_ground_to_stand_on() {
    // The first has sea at the origin, where players entered before a world's spawn
    // was chosen as the game chooses it; the others were not chosen.
    for seed in [SEED, 1, 2, 3, -4] {
        let generator = TerrainGenerator::new(seed);
        let spawn = generator.spawn();
        let chunk = generator.generate(spawn.chunk());
        let (x, z) = spawn.in_chunk();
        let below = chunk.get(x, spawn.y - 1, z).unwrap();
        let boxes: Vec<[f64; 6]> = below.props().collision_shape().boxes().collect();
        assert!(
            clustine_data::faces::face_full(&boxes, clustine_data::Face::Up),
            "seed {seed}: on {below:?}"
        );
        assert!(!below.has_fluid(), "seed {seed}: on {below:?}");
        assert!(
            !chunk.get(x, spawn.y, z).unwrap().has_fluid(),
            "seed {seed}: in a fluid"
        );
        // The same every time: a world keeps what it got, and a crash before it was
        // kept gets it again.
        assert_eq!(TerrainGenerator::new(seed).spawn(), spawn);
    }
}

/// The preview made on the scheduler's pool is the preview: the same chunk, lit the
/// same, marked the same, whichever chunk is asked for first and on any number of
/// threads.
#[test]
fn the_preview_on_the_scheduler_makes_the_chunks_the_preview_makes() {
    use std::sync::Arc;

    use clustine_worldgen::PreviewStages;
    use clustine_worldgen::scheduler::{Options, Order, Scheduler};

    let terrain = Arc::new(TerrainGenerator::new(SEED));
    let positions = [
        ChunkPos::new(0, 0),
        ChunkPos::new(1, 0),
        ChunkPos::new(-7, 12),
    ];
    for threads in [1, 4] {
        let scheduler = Scheduler::new(
            PreviewStages::new(Arc::clone(&terrain)),
            Options {
                threads,
                order: Order::Bare,
                ..Options::for_processors(threads)
            },
        );
        assert_eq!(scheduler.settings(), terrain.settings());
        for position in positions.iter().rev() {
            assert_eq!(scheduler.generate(*position), terrain.generate(*position));
        }
        let figures = scheduler.figures();
        assert_eq!((figures.starts, figures.runs), (0, 0));
    }
}
