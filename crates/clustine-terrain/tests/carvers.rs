//! The carvers without the game beside them: what has to hold of any chunk, and a
//! few chunks by what the game gave.
//!
//! The game's own classes judge the step in `tests/game_carvers.rs`, which needs
//! files that are not in the repository. What is here runs in the ordinary checks:
//! what the step may change and what not, that it is a function, and for a few
//! chunks the hash of their blocks, the number of blocks their mask names and the
//! number of their marks, each taken from what the game's classes wrote for that
//! chunk on 2026-10-13 (`tests/java/GameCarvers.java`; the chunks are the first of
//! the judge's lists for seed 13579), so that a change that alters a cave is seen
//! without the jar.

use clustine_data::blocks;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::{Filled, Terrain};
use clustine_terrain::surface::Biomes;

const SEED: i64 = 13579;

/// FNV-1a over the blocks' state ids, each as two bytes from the lowest.
fn hash(chunk: &Filled) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for state in &chunk.blocks {
        for byte in state.0.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

#[test]
fn carving_changes_only_what_a_mask_names_or_dirt_right_under_it_and_never_bedrock() {
    for (dimension, chunk) in [
        (Dimension::Overworld, (3, -7)),
        (Dimension::Nether, (0, 0)),
        (Dimension::End, (20, 20)),
    ] {
        let terrain = Terrain::new(SEED, dimension);
        let source = BiomeSource::new(SEED, dimension);
        let biomes = Biomes::from_source(&source, chunk.0, chunk.1);
        let surfaced = terrain.surface(chunk.0, chunk.1, &biomes);
        let carved = terrain.carved(chunk.0, chunk.1, &biomes, &source);
        // A function: the same again, and the mask alone is the chunk's mask.
        assert_eq!(carved, terrain.carved(chunk.0, chunk.1, &biomes, &source));
        assert_eq!(carved.mask, terrain.carving_mask(chunk.0, chunk.1, &source));
        if dimension == Dimension::End {
            // No biome of the End has a carver.
            assert!(carved.mask.is_empty());
            assert_eq!(carved.chunk, surfaced);
            continue;
        }
        let mut changed = 0;
        for (index, (before, after)) in surfaced.blocks.iter().zip(&carved.chunk.blocks).enumerate()
        {
            if before == after {
                continue;
            }
            changed += 1;
            let (x, z, y) = (
                index & 15,
                index >> 4 & 15,
                surfaced.min_y + (index >> 8) as i32,
            );
            assert_ne!(*before, blocks::BEDROCK);
            if carved.mask.get(x, y, z) {
                assert!(after.is_air() || after.has_fluid() || *before == blocks::DIRT);
            } else {
                // The top of the ground, put where dirt was under a carved block.
                assert_eq!(*before, blocks::DIRT);
                assert!(carved.mask.get(x, y + 1, z));
            }
        }
        assert!(changed > 0, "{dimension:?}: the chunk has caves");
        assert!(changed <= carved.mask.count() * 2);
        // Every mark of the surface is still there, and the marks are in order.
        for mark in &surfaced.marks {
            assert!(carved.chunk.marks.contains(mark));
        }
        assert!(carved.chunk.marks.is_sorted());
    }
}

#[test]
fn the_carvers_leave_the_lowest_block_and_the_top_of_the_terrain() {
    let overworld = Terrain::new(SEED, Dimension::Overworld);
    let heights = overworld.carver_heights();
    assert_eq!((heights.min_y, heights.depth), (-64, 384));
    // The Nether's terrain is half as high as its dimension, and the carvers read
    // the terrain's heights.
    let nether = Terrain::new(SEED, Dimension::Nether);
    let heights = nether.carver_heights();
    assert_eq!((heights.min_y, heights.depth), (0, 128));
    let source = BiomeSource::new(SEED, Dimension::Nether);
    for (x, z) in [(0, 0), (-1, -1), (5, 9)] {
        let mask = nether.carving_mask(x, z, &source);
        assert_eq!((mask.min_y(), mask.max_y()), (1, 120));
        for (_, _, lowest, highest) in mask.columns() {
            // An ellipsoid ends one above the lowest height a mask has.
            assert!(lowest >= 2 && highest <= 120);
        }
    }
}

#[test]
fn chunks_have_the_hashes_the_masks_and_the_marks_the_games_classes_gave() {
    // The chunk, the hash of its blocks, how many blocks its mask names, and how
    // many marks it has, a position counted as often as it is marked.
    let expected = [
        (
            Dimension::Overworld,
            0,
            0,
            0x6bc1_4b7a_c4ff_17df_u64,
            635,
            17,
        ),
        (Dimension::Overworld, -1, -1, 0x18da_42bf_e29a_62f9, 471, 1),
        // Reached by a canyon.
        (
            Dimension::Overworld,
            1147,
            -2082,
            0xcef9_1899_053f_27f8,
            1343,
            39,
        ),
        (Dimension::Nether, 0, 0, 0x7eac_afbe_6836_1405, 895, 0),
        (Dimension::Nether, -1, -1, 0xde49_3849_6a8c_2b18, 287, 0),
        (
            Dimension::Nether,
            -1403,
            2048,
            0xf454_dafb_2b7e_9848,
            8968,
            0,
        ),
    ];
    let made: Vec<(Dimension, i32, i32, u64, usize, usize)> = expected
        .iter()
        .map(|(dimension, x, z, _, _, _)| {
            let terrain = Terrain::new(SEED, *dimension);
            let source = BiomeSource::new(SEED, *dimension);
            let biomes = Biomes::from_source(&source, *x, *z);
            let carved = terrain.carved(*x, *z, &biomes, &source);
            (
                *dimension,
                *x,
                *z,
                hash(&carved.chunk),
                carved.mask.count(),
                carved.chunk.marks.len(),
            )
        })
        .collect();
    assert_eq!(made, expected, "{made:#x?}");
}
