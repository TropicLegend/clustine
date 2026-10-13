//! The biomes of chunks against hashes SteelMC's extractor took from the game: for
//! seed 13579 and each of the three dimensions, the MD5 of the names of a chunk's
//! biomes.
//!
//! The hashes are SteelMC's (`steel-core/test_assets/biome_hashes.json` at 885c4b3;
//! see `NOTICE` in this crate), and the way of hashing is that of its
//! `steel-core/tests/biome_hashes.rs`. A few are copied into this file and compared
//! in every run. All 2,500 chunks of each dimension are compared where a clone of
//! SteelMC is named, for its fixtures are read in the clone and not committed
//! (ADR-0019, section 2):
//!
//! ```text
//! CLUSTINE_STEELMC=<a clone of SteelMC at 885c4b3> \
//!     cargo test --release -p clustine-terrain -- --ignored --nocapture steelmc
//! ```
//!
//! **The fixture is not of the 26.3 jar this project is pinned to**, and where the two
//! part, the jar is followed. The jar's own classes (`tests/java/GameBiomes.java`, with
//! no code of this crate between) give another hash than the fixture for twelve of its
//! chunks of the overworld and 1,132 of the End, and the same for every chunk of the
//! Nether. Two causes were found: the End's islands (see
//! [`the_fixture_is_of_an_older_end`]), and the tree the game searches a parameter
//! list with, which has nineteen children a node in the jar and six in SteelMC, so
//! that some ties fall the other way. Those chunks are set aside here, by name; what
//! holds this crate to the jar there, cell by cell, is `tests/game.rs`.
//!
//! A hash says that a whole chunk is equal, and nothing about a cell. So a chunk
//! is made twice here. Once as this crate makes it, every cell a function of its
//! position. And once as the game makes it when it fills that chunk alone: in the
//! game's order of cells, each search starting from the entry found for the cell
//! before. One of the two must have the fixture's hash; and wherever the first
//! differs from the second, the two biomes must be exactly tied at that cell, which
//! is the only way this crate's biomes may differ from the game's
//! (`docs/groundwork/terrain-plan.md`, the rule of step T3).

use clustine_data::synced_registry;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_world::{BIOME_CELLS, Biome};
use md5::{Digest, Md5};

/// The seed of SteelMC's fixture.
const SEED: i64 = 13579;

/// Chunks of SteelMC's fixture with their hashes, copied from it: the chunk's x and
/// z, and the hash. The last two have the fixture's hash only as the game's memory
/// gives them: this crate decides a tie in them the other way.
const OVERWORLD: [(i32, i32, &str); 8] = [
    (-292_300, 122_122, "0afd36aaea63baf4aed1f51402397da8"),
    (-292_300, 122_123, "42e77f38c6c961e82173bd3c929e5fa8"),
    (-292_300, 122_124, "2cc439b73af1e86664956831732fccc2"),
    (-292_300, 122_128, "6165c91aa5cff91d4ff84361628a2c2a"),
    (-292_299, 122_122, "2df462518fb8f69a0dfce33786f50cfa"),
    (-292_299, 122_123, "8df0e02eabb65b56c404838379d89156"),
    (-181_254, -2_199, "e34e4fe19d79f20bdf9dc3ccc5412d8f"),
    (220_319, -100_312, "074dc2f38a06e6b509cb1f0471c554d5"),
];

/// As [`OVERWORLD`], for the Nether; the last has the fixture's hash only with the
/// game's memory.
const NETHER: [(i32, i32, &str); 6] = [
    (-292_300, 122_122, "a0f8841ebdb6c706262df06750702e75"),
    (-292_300, 122_130, "95afa94ee6a16759552c638dbf593610"),
    (-292_300, 122_131, "738beaabd2466b5e3a8b36c26e05423e"),
    (-292_299, 122_131, "00965969e908771439e682db4d2f619f"),
    (-181_261, -2_201, "f9b79a8b9f8709fa19ed42eb7d6f1161"),
    (338_467, -493_699, "d130079998d3f43257d70fd1209b2451"),
];

/// As [`OVERWORLD`], for the End: a chunk of each of the four biomes beyond the
/// central island, and last a chunk where the fixture is of an older End.
const END: [(i32, i32, &str); 5] = [
    (-292_300, 122_122, "6a34404721cf29bad27b7b63b3415668"),
    (-292_300, 122_123, ALL_BARRENS),
    (-292_299, 122_122, "1445a502d02eeee81906c8a0f7d48e07"),
    (-292_298, 122_123, "1f677a3e65d0ba99c779ab4baab844cb"),
    (-181_261, -2_201, ALL_BARRENS),
];

/// The hash of a chunk of the End that is `end_barrens` throughout.
const ALL_BARRENS: &str = "6e702161a5904d9294ec0623f2bba1d2";

/// The chunks of the overworld for which the 26.3 jar's own classes give another hash
/// than the fixture has (found by hashing what `tests/java/GameBiomes.java` wrote for
/// the fixture's chunks, in the fixture's order). In ten of them cells are exactly
/// tied; in the two at x 5355 none is, so there the game of the fixture gave another
/// biome than the jar does.
const NOT_THE_JARS: [(i32, i32); 12] = [
    (-292_298, 122_124),
    (220_324, -100_315),
    (220_324, -100_313),
    (429_646, -272_436),
    (188_864, -113_088),
    (5_355, -23_629),
    (5_355, -23_628),
    (300_243, 59_302),
    (421_420, 306_428),
    (421_421, 306_426),
    (421_425, 306_428),
    (421_426, 306_429),
];

/// Whether SteelMC's fixture has, for this chunk of the End, a value that the game
/// of 26.3 does not give.
///
/// Far from the origin the fixture has `end_barrens` in every chunk for which the
/// sum of the squares of the chunk's middle, counted in eighths of a chunk, does not
/// fit 32 bits and comes out negative: SteelMC's islands
/// (`steel-worldgen/src/noise/end_islands.rs`) take the root of that sum, which is
/// then no number, and no number is no island. In the data of the 26.3 jar the
/// distance from the origin is a density function of its own (`distance_to_point`,
/// in single precision, beside `end_outer_islands`), and nothing overflows: the
/// jar's own classes give islands in such chunks (`tests/game.rs` compares 2,500 of
/// them and more). So there this crate follows the jar and not the fixture, and the
/// test only holds the fixture to its `end_barrens`.
fn the_fixture_is_of_an_older_end(chunk_x: i32, chunk_z: i32) -> bool {
    let (x, z) = (chunk_x * 2 + 1, chunk_z * 2 + 1);
    x.wrapping_mul(x).wrapping_add(z.wrapping_mul(z)) < 0
}

/// The sections of a chunk, each its 64 cells in the order of `y << 4 | z << 2 | x`.
type Chunk = Vec<Vec<Biome>>;

/// SteelMC's hash of a chunk's biomes: for each section from the lowest, the low
/// byte of its section coordinate, then the names of its cells without their
/// namespace, in the order the cells are stored in.
fn hash(dimension: Dimension, chunk: &Chunk) -> String {
    let names = synced_registry("minecraft:worldgen/biome").unwrap().entries;
    let (lowest, _) = dimension.sections();
    let mut md5 = Md5::new();
    for (index, cells) in chunk.iter().enumerate() {
        md5.update([(lowest + index as i32) as u8]);
        for biome in cells {
            let name = names[usize::from(biome.0)];
            md5.update(name.trim_start_matches("minecraft:").as_bytes());
        }
    }
    md5.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The chunk as this crate makes it.
fn made(source: &BiomeSource, chunk_x: i32, chunk_z: i32) -> Chunk {
    source
        .chunk(chunk_x, chunk_z)
        .into_iter()
        .map(|cells| cells.to_vec())
        .collect()
}

/// The chunk as the game's search gives it when it fills the chunk: section by
/// section from the lowest, in each x, then y, then z, each search starting from the
/// entry found for the cell before, and the first from `before`, which is what the
/// thread found last in whichever chunk it filled earlier. The End has no search and
/// is as this crate makes it.
fn with_the_games_memory(
    source: &BiomeSource,
    chunk_x: i32,
    chunk_z: i32,
    mut before: Option<usize>,
) -> Chunk {
    let Some(tree) = source.parameters() else {
        return made(source, chunk_x, chunk_z);
    };
    let (lowest, section_count) = source.dimension().sections();
    let mut sections = vec![vec![Biome(0); BIOME_CELLS]; section_count];
    for (index, cells) in sections.iter_mut().enumerate() {
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    let climate = source.climate(
                        chunk_x * 4 + x as i32,
                        (lowest + index as i32) * 4 + y as i32,
                        chunk_z * 4 + z as i32,
                    );
                    let found = tree.nearest_after(&climate, before);
                    before = Some(found.entry);
                    cells[y << 4 | z << 2 | x] = found.biome;
                }
            }
        }
    }
    sections
}

/// The entries exactly tied at the first cell the game fills of a chunk. What the
/// thread remembers when it comes to a chunk changes the chunk only if it is one of
/// these: any other entry gives way at once.
fn entries_tied_at_the_first_cell(source: &BiomeSource, chunk_x: i32, chunk_z: i32) -> Vec<usize> {
    let Some(tree) = source.parameters() else {
        return Vec::new();
    };
    let (lowest, _) = source.dimension().sections();
    let climate = source.climate(chunk_x * 4, lowest * 4, chunk_z * 4);
    tree.tied(&climate)
        .into_iter()
        .map(|found| found.entry)
        .collect()
}

/// What the comparison of a dimension's chunks found.
#[derive(Default, Debug)]
struct Tally {
    chunks: usize,
    /// Chunks whose hash is the fixture's as this crate makes them.
    equal: usize,
    /// Chunks that have the fixture's hash only with the game's memory, when the
    /// chunk is filled by itself.
    equal_with_memory: Vec<(i32, i32)>,
    /// Chunks that have the fixture's hash only if the thread came to the chunk
    /// remembering another entry that is tied at the chunk's first cell.
    equal_with_memory_from_before: Vec<(i32, i32)>,
    /// Chunks that do not have the fixture's hash in any of these ways.
    unequal: Vec<(i32, i32)>,
    /// Chunks where the fixture is not the 26.3 jar's ([`NOT_THE_JARS`],
    /// [`the_fixture_is_of_an_older_end`]), which are not compared.
    set_aside: usize,
    /// Cells where this crate's biome is another than the game's memory gives.
    tied_cells: usize,
    /// Of those, the cells where the two biomes are not exactly tied. There must be
    /// none.
    untied_cells: Vec<(i32, i32, i32)>,
}

fn compare(source: &BiomeSource, chunk_x: i32, chunk_z: i32, expected: &str, tally: &mut Tally) {
    let dimension = source.dimension();
    let (lowest, _) = dimension.sections();
    tally.chunks += 1;
    if dimension == Dimension::End && the_fixture_is_of_an_older_end(chunk_x, chunk_z) {
        assert_eq!(
            expected, ALL_BARRENS,
            "the End's chunk {chunk_x}, {chunk_z}"
        );
        tally.set_aside += 1;
        return;
    }
    if dimension == Dimension::Overworld && NOT_THE_JARS.contains(&(chunk_x, chunk_z)) {
        tally.set_aside += 1;
        return;
    }

    let made = made(source, chunk_x, chunk_z);
    let remembering = with_the_games_memory(source, chunk_x, chunk_z, None);
    for (index, (ours, theirs)) in made.iter().zip(&remembering).enumerate() {
        for cell in (0..BIOME_CELLS).filter(|cell| ours[*cell] != theirs[*cell]) {
            let x = chunk_x * 4 + (cell & 3) as i32;
            let y = (lowest + index as i32) * 4 + (cell >> 4) as i32;
            let z = chunk_z * 4 + (cell >> 2 & 3) as i32;
            tally.tied_cells += 1;
            if !source.found(x, y, z).tied.contains(&theirs[cell]) {
                tally.untied_cells.push((x, y, z));
            }
        }
    }
    if hash(dimension, &made) == expected {
        tally.equal += 1;
    } else if hash(dimension, &remembering) == expected {
        tally.equal_with_memory.push((chunk_x, chunk_z));
    } else if entries_tied_at_the_first_cell(source, chunk_x, chunk_z)
        .into_iter()
        .any(|entry| {
            let from_before = with_the_games_memory(source, chunk_x, chunk_z, Some(entry));
            hash(dimension, &from_before) == expected
        })
    {
        tally.equal_with_memory_from_before.push((chunk_x, chunk_z));
    } else {
        tally.unequal.push((chunk_x, chunk_z));
    }
}

/// Holds a dimension's chunks to the fixture's hashes and gives what was found.
fn compare_all(dimension: Dimension, chunks: &[(i32, i32, &str)]) -> Tally {
    let source = BiomeSource::new(SEED, dimension);
    let mut tally = Tally::default();
    for (chunk_x, chunk_z, expected) in chunks {
        compare(&source, *chunk_x, *chunk_z, expected, &mut tally);
    }
    tally
}

#[test]
fn chunks_of_the_overworld_have_the_hashes_steelmc_took_from_the_game() {
    let tally = compare_all(Dimension::Overworld, &OVERWORLD);
    assert!(
        tally.unequal.is_empty() && tally.untied_cells.is_empty(),
        "{tally:?}"
    );
    // The chunks were chosen so that both ways of being equal are met.
    assert_eq!(tally.equal, 6, "{tally:?}");
    assert_eq!(tally.equal_with_memory.len(), 2, "{tally:?}");
    assert!(tally.tied_cells > 0);
}

#[test]
fn chunks_of_the_nether_have_the_hashes_steelmc_took_from_the_game() {
    let tally = compare_all(Dimension::Nether, &NETHER);
    assert!(
        tally.unequal.is_empty() && tally.untied_cells.is_empty(),
        "{tally:?}"
    );
    assert_eq!(tally.equal, 5, "{tally:?}");
    assert_eq!(tally.equal_with_memory.len(), 1, "{tally:?}");
}

#[test]
fn chunks_of_the_end_have_the_hashes_steelmc_took_from_the_game() {
    let tally = compare_all(Dimension::End, &END);
    assert!(tally.unequal.is_empty(), "{tally:?}");
    assert_eq!((tally.equal, tally.set_aside), (4, 1), "{tally:?}");
}

#[test]
#[ignore = "needs a clone of SteelMC, and a release build to be quick"]
fn every_chunk_of_steelmcs_fixture_has_its_hash() {
    let Ok(clone) = std::env::var("CLUSTINE_STEELMC") else {
        println!("skipped: CLUSTINE_STEELMC names no clone of SteelMC");
        return;
    };
    let path = std::path::Path::new(&clone).join("steel-core/test_assets/biome_hashes.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(fixture["seed"].as_i64(), Some(SEED));

    let mut failed = false;
    for (dimension, name) in [
        (Dimension::Overworld, "overworld"),
        (Dimension::Nether, "the_nether"),
        (Dimension::End, "the_end"),
    ] {
        let of = &fixture[name];
        let (lowest, count) = dimension.sections();
        assert_eq!(of["min_section_y"].as_i64(), Some(i64::from(lowest)));
        assert_eq!(
            of["max_section_y"].as_i64(),
            Some(i64::from(lowest) + count as i64 - 1)
        );

        let chunks: Vec<(i32, i32, &str)> = of["hashes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry[0].as_i64().unwrap() as i32,
                    entry[1].as_i64().unwrap() as i32,
                    entry[2].as_str().unwrap(),
                )
            })
            .collect();
        let distinct: std::collections::BTreeSet<&str> =
            chunks.iter().map(|chunk| chunk.2).collect();
        let tally = compare_all(dimension, &chunks);
        println!(
            "{name}: {} chunks with {} distinct hashes: {} set aside as not the jar's; {} equal \
             as made, {} more with the game's memory within the chunk, {} more with its memory \
             from before the chunk, {} unequal; {} cells differ with and without the memory, {} \
             of them not tied",
            tally.chunks,
            distinct.len(),
            tally.set_aside,
            tally.equal,
            tally.equal_with_memory.len(),
            tally.equal_with_memory_from_before.len(),
            tally.unequal.len(),
            tally.tied_cells,
            tally.untied_cells.len(),
        );
        for (chunk_x, chunk_z) in tally.unequal.iter().take(20) {
            println!("  unequal: the chunk {chunk_x}, {chunk_z}");
        }
        for (x, y, z) in tally.untied_cells.iter().take(20) {
            println!("  not tied: the cell {x}, {y}, {z}");
        }
        failed |= !tally.unequal.is_empty() || !tally.untied_cells.is_empty();
    }
    assert!(
        !failed,
        "chunks differ from SteelMC's fixture by more than exact ties"
    );
}
