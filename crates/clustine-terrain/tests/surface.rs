//! The surface without the game beside it: what has to hold of any chunk, and a few
//! chunks by their hashes.
//!
//! The game's own classes judge the step in `tests/game_surface.rs`, which needs
//! files that are not in the repository. What is here runs in the ordinary checks:
//! the layers that are easy to get wrong (bedrock at the bottom, deepslate up to the
//! height 8, the Nether's roof), that the step is a function, that the marks are
//! kept, and hashes of chunks that the judge compared block by block on 2026-10-13
//! (the origin of each dimension for seed 13579 is in its lists), so that a change
//! that alters a surface is seen without the jar.

use clustine_data::{BlockState, blocks};
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::{Filled, Terrain};
use clustine_terrain::noise_chunk::Scratch;
use clustine_terrain::surface::Biomes;

const SEED: i64 = 13579;

fn surface(dimension: Dimension, x: i32, z: i32) -> (Filled, Filled) {
    let terrain = Terrain::new(SEED, dimension);
    let source = BiomeSource::new(SEED, dimension);
    let biomes = Biomes::from_source(&source, x, z);
    (terrain.fill(x, z), terrain.surface(x, z, &biomes))
}

/// How many blocks of a layer are `state`.
fn in_layer(chunk: &Filled, y: i32, state: BlockState) -> usize {
    (0..256)
        .filter(|column| chunk.get(column & 15, y, column >> 4) == state)
        .count()
}

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
fn the_overworld_has_bedrock_in_its_bottom_five_layers_thinning_upwards() {
    let mut layers = [0; 6];
    for (x, z) in [(0, 0), (3, -7), (-20, 14), (100, 100)] {
        let (_, chunk) = surface(Dimension::Overworld, x, z);
        for (up, layer) in layers.iter_mut().enumerate() {
            *layer += in_layer(&chunk, -64 + up as i32, blocks::BEDROCK);
        }
    }
    // The lowest layer is bedrock throughout, the sixth has none, and between them
    // each has less than the one below: about four fifths, three fifths and so on.
    assert_eq!(layers[0], 4 * 256);
    assert_eq!(layers[5], 0);
    for up in 1..5 {
        assert!(layers[up] < layers[up - 1], "{layers:?}");
        assert!(layers[up] > 0, "{layers:?}");
    }
    assert!((700..940).contains(&layers[1]), "{layers:?}");
    assert!((100..340).contains(&layers[4]), "{layers:?}");
}

#[test]
fn stone_is_deepslate_up_to_zero_and_stone_from_eight() {
    let (filled, chunk) = surface(Dimension::Overworld, 100, 100);
    let stone = blocks::STONE;
    let deepslate = blocks::DEEPSLATE;
    let mut mixed = [0; 8];
    for column in 0..256_usize {
        let (x, z) = (column & 15, column >> 4);
        for y in -58..40 {
            if filled.get(x, y, z) != stone {
                continue;
            }
            let after = chunk.get(x, y, z);
            // What is neither is an ore vein or the surface.
            if y <= 0 {
                assert_ne!(after, stone, "{x} {y} {z}");
            }
            if y >= 8 {
                assert_ne!(after, deepslate, "{x} {y} {z}");
            }
            if (1..8).contains(&y) && after == deepslate {
                mixed[y as usize] += 1;
            }
        }
    }
    // Between the two the deepslate thins out upwards.
    assert!(mixed[1] > mixed[7], "{mixed:?}");
    assert!(mixed[7] > 0, "{mixed:?}");
}

#[test]
fn the_nether_has_bedrock_at_its_bottom_and_its_roof() {
    let mut floor = [0; 6];
    let mut roof = [0; 6];
    let mut between = 0;
    for (x, z) in [(0, 0), (3, -7), (-20, 14)] {
        let (_, chunk) = surface(Dimension::Nether, x, z);
        for up in 0..6 {
            floor[up] += in_layer(&chunk, up as i32, blocks::BEDROCK);
            roof[up] += in_layer(&chunk, 127 - up as i32, blocks::BEDROCK);
        }
        for y in 6..122 {
            between += in_layer(&chunk, y, blocks::BEDROCK);
        }
        // Nothing of the Nether's terrain is above its roof.
        assert_eq!(chunk.height, 128);
    }
    assert_eq!(floor[0], 3 * 256);
    assert_eq!(roof[0], 3 * 256);
    assert_eq!((floor[5], roof[5], between), (0, 0, 0));
    for up in 1..5 {
        assert!(floor[up] < floor[up - 1] && floor[up] > 0, "{floor:?}");
        assert!(roof[up] < roof[up - 1] && roof[up] > 0, "{roof:?}");
    }
}

#[test]
fn the_end_stays_what_the_fill_made_it() {
    for (x, z) in [(0, 0), (3, -2), (-40, 0), (1000, 1000)] {
        let (filled, chunk) = surface(Dimension::End, x, z);
        assert_eq!(filled, chunk);
    }
}

#[test]
fn the_surface_is_a_function_and_keeps_the_fills_marks() {
    let terrain = Terrain::new(SEED, Dimension::Overworld);
    let source = BiomeSource::new(SEED, Dimension::Overworld);
    for (x, z) in [(0, 0), (-20, 14)] {
        let biomes = Biomes::from_source(&source, x, z);
        let filled = terrain.fill(x, z);
        let first = terrain.surface(x, z, &biomes);
        // Other chunks in between, and the step alone on a chunk that was filled
        // before, with room that an earlier chunk used.
        let mut scratch = Scratch::default();
        let mut other = terrain.fill(x + 1, z);
        terrain.build_surface(
            &mut other,
            x + 1,
            z,
            &Biomes::from_source(&source, x + 1, z),
            &mut scratch,
        );
        let mut again = filled.clone();
        terrain.build_surface(&mut again, x, z, &biomes, &mut scratch);
        assert_eq!(first, again);

        // Every mark of the fill is still there, and the marks are in order.
        for mark in &filled.marks {
            assert!(first.marks.contains(mark));
        }
        assert!(first.marks.is_sorted());
        // What the step changed is ground: no air and no fluid of the fill.
        for (before, after) in filled.blocks.iter().zip(&first.blocks) {
            if before.is_air() || before.has_fluid() {
                assert_eq!(before, after);
            }
        }
    }
}

#[test]
fn the_biome_at_a_block_is_that_of_a_cell_beside_it() {
    let terrain = Terrain::new(SEED, Dimension::Overworld);
    let source = BiomeSource::new(SEED, Dimension::Overworld);
    // A chunk where two biomes meet (the origin is desert beside a river).
    let biomes = Biomes::from_source(&source, 0, 0);
    let mut kinds = Vec::new();
    for x in 0..16 {
        for z in 0..16 {
            for y in [-64, 0, 63, 100, 319] {
                let (cell_x, cell_y, cell_z) = terrain.zoom().cell(x, y, z);
                let biome = biomes.at_block(terrain.zoom(), x, y, z);
                assert_eq!(biome, biomes.cell(cell_x, cell_y, cell_z));
                if !kinds.contains(&biome) {
                    kinds.push(biome);
                }
            }
        }
    }
    assert!(kinds.len() >= 2, "{kinds:?}");
}

#[test]
fn the_top_material_of_a_block_under_the_sky_is_what_the_surface_put_there() {
    // The game's own judges it in `tests/game_carvers.rs`; this holds it to the
    // step itself where the two must agree: a block of grass under the sky.
    // (They need not agree everywhere: the game asks for the top material as if the
    // block had air right below it too, so that sand comes out as sandstone.)
    let terrain = Terrain::new(SEED, Dimension::Overworld);
    let source = BiomeSource::new(SEED, Dimension::Overworld);
    let mut asked = 0;
    for x in 0..60 {
        let z = -7;
        let biomes = Biomes::from_source(&source, x, z);
        let mut chunk = terrain.surface(x, z, &biomes);
        for column in 0..256_usize {
            let (in_x, in_z) = (column & 15, column >> 4);
            let Some(top) = (chunk.min_y..chunk.min_y + chunk.height as i32)
                .rev()
                .find(|y| !chunk.get(in_x, *y, in_z).is_air())
            else {
                continue;
            };
            let state = chunk.get(in_x, top, in_z);
            if state != blocks::GRASS_BLOCK {
                continue;
            }
            let material = terrain.top_material(
                &mut chunk,
                &biomes,
                x * 16 + in_x as i32,
                top,
                z * 16 + in_z as i32,
                false,
            );
            assert_eq!(
                material,
                Some(state),
                "chunk {x} {z}, column {in_x} {in_z} at {top}"
            );
            asked += 1;
        }
        if asked > 200 {
            break;
        }
    }
    assert!(asked > 200, "{asked}");
}

#[test]
fn chunks_have_the_hashes_of_what_the_game_was_compared_with() {
    let expected = [
        (Dimension::Overworld, 0, 0, 0x9a14_2279_70b6_575f_u64),
        (Dimension::Overworld, 3, -7, 0xd39b_72d4_ac10_b9ac),
        (Dimension::Nether, 0, 0, 0xbe18_6dfb_196a_96d9),
        (Dimension::Nether, 3, -7, 0x0d7f_c3be_5be9_ce05),
        (Dimension::End, 0, 0, 0x444c_6ed7_f0a2_5e51),
    ];
    let made: Vec<(Dimension, i32, i32, u64)> = expected
        .iter()
        .map(|(dimension, x, z, _)| (*dimension, *x, *z, hash(&surface(*dimension, *x, *z).1)))
        .collect();
    assert_eq!(made, expected, "{made:#x?}");
}
