//! The fill of a few chunks against known values of the game's.
//!
//! The values in this file were computed by the classes of the pinned server jar
//! (`tests/java/GameFill.java`, which has the game's generator do its "fill from noise"
//! step without a server) and copied out of what it wrote: for a chunk the hash of the
//! bits of its final density, how many blocks of each state it has, the positions the
//! game marks, the height at which its aquifer stops asking, and columns the game
//! made alone. They are Mojang's data and not under the licence of this file (see
//! NOTICE.md at the root of the repository, and `NOTICE` in this crate).
//!
//! `tests/game_fill.rs` holds every block of some thousand chunks to the same program;
//! it needs the jar, and this does not.

use std::collections::BTreeMap;

use clustine_data::{BlockState, DIMENSION_TYPES, blocks};
use clustine_terrain::biome::Dimension;
use clustine_terrain::fill::{Heightmap, Terrain};
use clustine_terrain::noise_chunk::Scratch;
use clustine_world::{Biome, InChunk};

/// The hash `GameFill.java` takes of a density: FNV-1a over each value's bits, the
/// lowest byte first.
fn hash(density: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in density {
        for byte in value.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// A column from its runs of `(state, count)`, from the bottom.
fn column(runs: &[(u16, usize)]) -> Vec<BlockState> {
    runs.iter()
        .flat_map(|(state, count)| std::iter::repeat_n(BlockState(*state), *count))
        .collect()
}

/// What the game gave for a chunk.
struct Known {
    seed: i64,
    dimension: Dimension,
    chunk: (i32, i32),
    density: u64,
    /// How many blocks of each state.
    counts: &'static [(u16, usize)],
    /// How many positions are marked.
    marks: usize,
    ceiling: Option<i32>,
}

const STONE: u16 = 1;
const WATER: u16 = 89;
const LAVA: u16 = 105;
const NETHERRACK: u16 = 8667;
const END_STONE: u16 = 11211;

const KNOWN: [Known; 8] = [
    Known {
        seed: 13579,
        dimension: Dimension::Overworld,
        chunk: (0, 0),
        density: 0x0e30_a79f_7355_9683,
        counts: &[(0, 69034), (STONE, 27258), (WATER, 2012)],
        marks: 17,
        ceiling: Some(82),
    },
    // A chunk in which the game's volume of corners is not what the game gives for
    // each corner alone: a noise of the noodle caves differs in its last bits over
    // one plane of corners, and with it the density of some hundred blocks.
    Known {
        seed: 13579,
        dimension: Dimension::Overworld,
        chunk: (626, 1424),
        density: 0x73b8_b17f_926e_8e3c,
        counts: &[(0, 45237), (STONE, 53067)],
        marks: 0,
        ceiling: Some(178),
    },
    // Near the world's edge.
    Known {
        seed: 13579,
        dimension: Dimension::Overworld,
        chunk: (1_874_990, 0),
        density: 0xd96c_49fc_a6e2_6184,
        counts: &[(0, 65576), (STONE, 32728)],
        marks: 0,
        ceiling: Some(94),
    },
    Known {
        seed: -7_046_029_254_386_353_131,
        dimension: Dimension::Overworld,
        chunk: (0, 0),
        density: 0x2a0b_a3e6_8fa4_9fec,
        counts: &[(0, 73052), (STONE, 25252)],
        marks: 0,
        ceiling: Some(94),
    },
    Known {
        seed: 13579,
        dimension: Dimension::Nether,
        chunk: (0, 0),
        density: 0x6b6c_c35a_d52a_beb3,
        counts: &[(0, 7783), (LAVA, 2522), (NETHERRACK, 22463)],
        marks: 0,
        ceiling: None,
    },
    // Ten million blocks out.
    Known {
        seed: 13579,
        dimension: Dimension::Nether,
        chunk: (625_000, 0),
        density: 0x4e53_396b_9b27_2beb,
        counts: &[(0, 12908), (LAVA, 2066), (NETHERRACK, 17794)],
        marks: 0,
        ceiling: None,
    },
    Known {
        seed: 13579,
        dimension: Dimension::End,
        chunk: (0, 0),
        density: 0x497b_a5d8_11e6_3a8c,
        counts: &[(0, 20834), (END_STONE, 11934)],
        marks: 0,
        ceiling: None,
    },
    // Among the outer islands, far out.
    Known {
        seed: 13579,
        dimension: Dimension::End,
        chunk: (-418_462, 366_791),
        density: 0x5090_1f03_ae81_6031,
        counts: &[(0, 26070), (END_STONE, 6698)],
        marks: 0,
        ceiling: None,
    },
];

#[test]
fn chunks_have_the_density_the_blocks_and_the_marks_the_game_gives() {
    for known in &KNOWN {
        let name = format!(
            "{:?}, seed {}, chunk {:?}",
            known.dimension, known.seed, known.chunk
        );
        let terrain = Terrain::new(known.seed, known.dimension);
        let (filled, density) =
            terrain.fill_with_density(known.chunk.0, known.chunk.1, &mut Scratch::default());
        assert_eq!(hash(&density), known.density, "{name}: the density");

        let mut counts: BTreeMap<u16, usize> = BTreeMap::new();
        for state in &filled.blocks {
            *counts.entry(state.0).or_default() += 1;
        }
        let counts: Vec<(u16, usize)> = counts.into_iter().collect();
        assert_eq!(counts, known.counts, "{name}: the blocks");
        assert_eq!(filled.marks.len(), known.marks, "{name}: the marks");
        assert_eq!(
            terrain.aquifer_ceiling(known.chunk.0, known.chunk.1),
            known.ceiling,
            "{name}: where the aquifer stops asking"
        );
    }
}

#[test]
fn the_marks_of_the_chunk_at_the_origin_are_the_games() {
    // In the order the game made them: column by column, from the top.
    let theirs = [
        (6, 31, 13),
        (7, 31, 13),
        (10, 30, 14),
        (1, 22, 15),
        (1, 21, 15),
        (2, 22, 15),
        (9, 31, 15),
        (10, 30, 15),
        (11, 31, 15),
        (4, 32, 13),
        (5, 32, 13),
        (7, 32, 14),
        (8, 32, 14),
        (12, 32, 14),
        (7, 32, 15),
        (13, 33, 15),
        (14, 33, 15),
    ];
    let mut theirs: Vec<InChunk> = theirs
        .iter()
        .map(|(x, y, z)| InChunk::new(*x, *y, *z))
        .collect();
    theirs.sort_unstable();
    let filled = Terrain::new(13579, Dimension::Overworld).fill(0, 0);
    assert_eq!(filled.marks, theirs);
    // Every mark is on a fluid.
    for mark in &filled.marks {
        let state = filled.get(usize::from(mark.x), i32::from(mark.y), usize::from(mark.z));
        assert_eq!(state, blocks::WATER);
    }
}

#[test]
fn columns_made_alone_are_the_games() {
    // The seed, the dimension, the block's x and z, the world surface the game's
    // `getBaseHeight` gives, and the column of the game's `getBaseColumn`.
    type Alone = (i64, Dimension, i32, i32, i32, &'static [(u16, usize)]);
    let columns: [Alone; 10] = [
        (
            13579,
            Dimension::Overworld,
            0,
            0,
            63,
            &[(STONE, 56), (0, 16), (STONE, 47), (WATER, 8), (0, 257)],
        ),
        (
            13579,
            Dimension::Overworld,
            5,
            9,
            63,
            &[(STONE, 57), (0, 12), (STONE, 48), (WATER, 10), (0, 257)],
        ),
        (
            13579,
            Dimension::Overworld,
            -6_695_389,
            5_868_663,
            69,
            &[
                (STONE, 58),
                (0, 13),
                (STONE, 34),
                (0, 3),
                (STONE, 25),
                (0, 251),
            ],
        ),
        (
            13579,
            Dimension::Overworld,
            29_999_847,
            15,
            64,
            &[(STONE, 128), (0, 256)],
        ),
        (
            -7_046_029_254_386_353_131,
            Dimension::Overworld,
            0,
            0,
            68,
            &[
                (STONE, 15),
                (0, 35),
                (STONE, 62),
                (0, 7),
                (STONE, 13),
                (0, 252),
            ],
        ),
        (
            13579,
            Dimension::Nether,
            0,
            0,
            128,
            &[
                (NETHERRACK, 22),
                (LAVA, 10),
                (0, 22),
                (NETHERRACK, 30),
                (0, 16),
                (NETHERRACK, 28),
            ],
        ),
        (
            13579,
            Dimension::Nether,
            10_000_013,
            1,
            128,
            &[
                (NETHERRACK, 23),
                (LAVA, 8),
                (NETHERRACK, 30),
                (0, 33),
                (NETHERRACK, 3),
                (0, 12),
                (NETHERRACK, 19),
            ],
        ),
        (
            13579,
            Dimension::End,
            0,
            0,
            62,
            &[(0, 14), (END_STONE, 48), (0, 66)],
        ),
        (
            13579,
            Dimension::End,
            5,
            9,
            61,
            &[(0, 15), (END_STONE, 46), (0, 67)],
        ),
        (
            13579,
            Dimension::End,
            -6_695_384,
            5_868_656,
            57,
            &[(0, 24), (END_STONE, 16), (0, 7), (END_STONE, 10), (0, 71)],
        ),
    ];
    for (seed, dimension, x, z, world_surface, runs) in columns {
        let name = format!("{dimension:?}, seed {seed}, column {x} {z}");
        let terrain = Terrain::new(seed, dimension);
        let theirs = column(runs);
        assert_eq!(terrain.base_column(x, z), theirs, "{name}");
        assert_eq!(
            terrain.base_height(x, z, Heightmap::WorldSurfaceWg),
            world_surface,
            "{name}: the world surface"
        );
        // The ocean floor is the highest block that is neither air nor a fluid here.
        let floor = theirs
            .iter()
            .rposition(|state| !state.is_air() && !state.has_fluid())
            .map_or(terrain.settings().min_y, |y| {
                terrain.settings().min_y + y as i32 + 1
            });
        assert_eq!(
            terrain.base_height(x, z, Heightmap::OceanFloorWg),
            floor,
            "{name}: the ocean floor"
        );
    }
}

#[test]
fn where_no_aquifer_is_a_column_made_alone_is_the_column_of_its_filled_chunk() {
    // The game makes an aquifer for the column alone, which stops asking at another
    // height than the chunk's. Where the two columns differ is the game's to say;
    // that the fill and the column agree on the ground where no aquifer is, is
    // checked here for the Nether and the End.
    for dimension in [Dimension::Nether, Dimension::End] {
        let terrain = Terrain::new(13579, dimension);
        let filled = terrain.fill(3, -7);
        for (x, z) in [(0, 0), (5, 9), (15, 15)] {
            let alone = terrain.base_column(3 * 16 + x, -7 * 16 + z);
            for (y, state) in alone.iter().enumerate() {
                assert_eq!(
                    filled.get(x as usize, filled.min_y + y as i32, z as usize),
                    *state
                );
            }
        }
    }
}

#[test]
fn a_filled_chunk_becomes_a_chunk_of_the_world_with_its_marks() {
    let nether_type = DIMENSION_TYPES
        .iter()
        .find(|kind| kind.name == Dimension::Nether.type_name())
        .unwrap();
    let filled = Terrain::new(13579, Dimension::Nether).fill(0, 0);
    assert_eq!((filled.min_y, filled.height), (0, 128));
    let chunk = filled.clone().into_chunk(nether_type, Biome(0));
    // The Nether's terrain is half as high as its dimension: air above.
    assert_eq!(chunk.height(), 256);
    for (x, y, z) in [
        (0, 0, 0),
        (0, 25, 0),
        (0, 40, 0),
        (7, 127, 9),
        (15, 100, 15),
    ] {
        assert_eq!(chunk.get(x, y, z), Some(filled.get(x, y, z)), "{x} {y} {z}");
    }
    assert_eq!(chunk.get(3, 128, 3), Some(blocks::AIR));
    assert_eq!(chunk.get(3, 255, 3), Some(blocks::AIR));
    assert_eq!(filled.get(3, 128, 3), blocks::AIR);
    assert_eq!(filled.get(3, -1, 3), blocks::AIR);
    assert!(chunk.marks().is_empty());

    let overworld_type = DIMENSION_TYPES
        .iter()
        .find(|kind| kind.name == Dimension::Overworld.type_name())
        .unwrap();
    let filled = Terrain::new(13579, Dimension::Overworld).fill(0, 0);
    let marks = filled.marks.clone();
    let chunk = filled.into_chunk(overworld_type, Biome(0));
    assert_eq!(chunk.marks(), marks);
    assert_eq!(chunk.get(0, 62, 0), Some(blocks::WATER));
    assert_eq!(chunk.get(0, -64, 0), Some(blocks::STONE));
}

#[test]
fn the_fill_is_a_function_of_the_seed_and_the_chunk() {
    // Whatever was filled before, and from whichever instance.
    let terrain = Terrain::new(13579, Dimension::Overworld);
    let first = terrain.fill(3, -7);
    let _ = terrain.fill(4, -7);
    let _ = terrain.base_height(50, -110, Heightmap::WorldSurfaceWg);
    assert_eq!(terrain.fill(3, -7), first);
    assert_eq!(Terrain::new(13579, Dimension::Overworld).fill(3, -7), first);
    assert_ne!(Terrain::new(13580, Dimension::Overworld).fill(3, -7), first);
}
