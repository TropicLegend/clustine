//! The fill, the surface and the carvers against hashes SteelMC's extractor took from
//! the game: for seed 13579, the MD5 of a chunk's blocks at the game's `terrain`
//! status.
//!
//! The hashes are SteelMC's (`steel-core/test_assets/chunk_stage_hashes.json` at
//! 885c4b3; see `NOTICE` in this crate), and the way of hashing is that of its
//! `steel-core/src/worldgen/chunk_stage_hashes.rs`. A few are copied into this file
//! and compared in every run. All 2,500 chunks of each dimension are compared where a
//! clone of SteelMC is named, for its fixtures are read in the clone and not committed
//! (ADR-0019, section 2):
//!
//! ```text
//! CLUSTINE_STEELMC=<a clone of SteelMC at 885c4b3> \
//!     cargo test --release -p clustine-terrain -- --ignored --nocapture steelmc
//! ```
//!
//! **Only the End is judged by them.** A chunk at the `terrain` status has been
//! filled, has had its surface made by the dimension's material rule, and has been
//! carved; and structures near it have bent its ground. The first three are built:
//!
//! - *The End*: its material rule puts end stone where end stone is, no biome of it
//!   has a carver, and no structure of it bends the ground. So a chunk of the End at
//!   `terrain` is the chunk after the fill, and the fixture's hash is of the fill.
//! - *The overworld and the Nether*: after the carvers come the structures that bend
//!   the ground (step T7b of `docs/groundwork/terrain-plan.md`), and nothing in the
//!   fixture says which chunks those reach. So the test counts the chunks that, made
//!   with this crate's own biomes, have the fixture's hash once they are carved, and
//!   judges nothing by them: a chunk that has not may have had its ground bent, or
//!   hold a biome that is an exact tie (`clustine_terrain::biome`).
//!
//! **What it gave**, on 2026-10-13: all 2,500 chunks of the End have the fixture's
//! hash; 1,008 of them are empty. That includes the 1,300 chunks far out for which the
//! same fixture's *biomes* are not what the 26.3 jar gives (`tests/steelmc_hashes.rs`
//! says why): the blocks of its `terrain` status are the jar's there too. Of the
//! overworld's 2,500 chunks 2,444 have the fixture's hash once they are carved, 2,235
//! of them chunks that a carver changed (210 had it after the surface alone); of the
//! Nether's, 2,230, of which a carver changed 1,837 (393 after the surface). A few of
//! each kind are copied into this file.
//!
//! **The 56 and 270 that have not the hash** lie together, as chunks do whose ground
//! one structure bent: of the fixture's chunks around them more than half have not
//! the hash either, and only five of the overworld's and one of the Nether's have no
//! such neighbour (three of those five hold a biome that is an exact tie, the other
//! three lie at the rim of one of the fixture's squares). And it is not the fill,
//! the surface or the carvers that differ in them: where `CLUSTINE_STEELMC_DIFFERENT`
//! names a directory, the test writes these chunks as lists for
//! `tests/java/GameCarvers.java`, and the game's own three steps without structures
//! gave all 326 of them block for block as this crate makes them
//! (`tests/game_carvers.rs`, 14,352,384 blocks; with the game's biomes, which differ
//! from this crate's in five of them, at ties). So the fixture's chunks hold
//! something the three steps do not make. That it is the ground structures bent
//! cannot be shown before structures have starts (T7b).

use clustine_data::BlockState;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::{Filled, Terrain};
use clustine_terrain::surface::Biomes;
use md5::{Digest, Md5};

/// The seed of SteelMC's fixture.
const SEED: i64 = 13579;

/// Chunks of the End of SteelMC's fixture with their hashes at `terrain`, copied
/// from it: the chunk's x and z, and the hash. The fifth is empty; the last is the
/// fixture's nearest to the origin.
const END: [(i32, i32, &str); 6] = [
    (-292_300, 122_122, "02269c10103a5d57eb9e1081563c5ef4"),
    (-292_300, 122_123, "d5fa49604dbc576d20796bd07728095d"),
    (-292_299, 122_122, "e8a232673f573c930bc70324cf60cbd7"),
    (-292_298, 122_123, "8bdfc17d683dd8d760bc672c0dfa2be3"),
    (-181_261, -2_201, EMPTY),
    (-2_454, 11_929, "b390a4b2d9e5242a43c800e8e1982eee"),
];

/// Chunks of the overworld of SteelMC's fixture with their hashes at `terrain`. The
/// first three have that hash once their surface is made: no carver and no structure
/// touched them. The last three have it only once they are carved.
const OVERWORLD: [(i32, i32, &str); 6] = [
    (-408_525, -480_569, "0b51d8adbe8ba3a1e5fe0285f6780107"),
    (-408_525, -480_568, "6d973cc4367b61d7e9edaf1d0c8a4302"),
    (-408_525, -480_566, "421c3e7b1314796b4339397877b69ae7"),
    (-418_462, 366_791, "77e34064b619a01f535d7feabba2717e"),
    (-418_462, 366_799, "1f79a1d004ce85ca4f35739e188b4749"),
    (-418_462, 366_800, "f25aa82cdcab53039b667fc9cb7aa1ba"),
];

/// As [`OVERWORLD`], for the Nether.
const NETHER: [(i32, i32, &str); 6] = [
    (-418_461, 366_798, "3457f4b818f0ddc645b53aa6089ef0a2"),
    (-418_461, 366_800, "3831a6fff24db3620211ed168aa55ee2"),
    (-418_460, 366_797, "17255f2b51b23c361399858ba747ddde"),
    (-418_462, 366_791, "de2de03d54f421ce52b152e4a3c978ed"),
    (-418_462, 366_792, "57e4cab7260e73828c1c9c2f84abedc8"),
    (-418_462, 366_793, "9413bdca6fb1431dffd07d1055c4393b"),
];

/// The hash of a chunk of the End without a block.
const EMPTY: &str = "4ae71336e44bf9bf79d2752e234818a5";

/// SteelMC's hash of a chunk's blocks: for each section of the dimension from the
/// lowest, one zero byte if every block of it is air, else every block's state as
/// four bytes from the highest, in the order of y, z, x.
fn hash(dimension: Dimension, filled: &Filled) -> String {
    let (lowest, count) = dimension.sections();
    let mut md5 = Md5::new();
    for section in 0..count {
        let bottom = (lowest + section as i32) * 16;
        let mut states: Vec<BlockState> = Vec::with_capacity(4096);
        for y in bottom..bottom + 16 {
            for z in 0..16 {
                for x in 0..16 {
                    states.push(filled.get(x, y, z));
                }
            }
        }
        if states.iter().all(|state| state.is_air()) {
            md5.update([0_u8]);
        } else {
            for state in states {
                md5.update(u32::from(state.0).to_be_bytes());
            }
        }
    }
    md5.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn chunks_of_the_end_have_the_hashes_of_steelmcs_fixture() {
    let terrain = Terrain::new(SEED, Dimension::End);
    for (x, z, theirs) in END {
        assert_eq!(
            hash(Dimension::End, &terrain.fill(x, z)),
            theirs,
            "the End's chunk {x} {z}"
        );
    }
}

#[test]
fn chunks_have_the_hashes_of_steelmcs_fixture_after_the_surface_or_once_they_are_carved() {
    for (dimension, chunks) in [
        (Dimension::Overworld, OVERWORLD),
        (Dimension::Nether, NETHER),
    ] {
        let terrain = Terrain::new(SEED, dimension);
        let source = BiomeSource::new(SEED, dimension);
        for (place, (x, z, theirs)) in chunks.into_iter().enumerate() {
            let biomes = Biomes::from_source(&source, x, z);
            let surface = terrain.surface(x, z, &biomes);
            let carved = terrain.carved(x, z, &biomes, &source);
            assert_eq!(
                hash(dimension, &carved.chunk),
                theirs,
                "{dimension:?}, chunk {x} {z}"
            );
            // The first three are what the surface left, the others are not.
            assert_eq!(
                hash(dimension, &surface) == theirs,
                place < 3,
                "{dimension:?}, chunk {x} {z}"
            );
        }
    }
}

#[test]
#[ignore = "needs a clone of SteelMC, named by CLUSTINE_STEELMC"]
fn every_chunk_of_the_end_of_steelmcs_fixture_has_its_hash() {
    let Ok(clone) = std::env::var("CLUSTINE_STEELMC") else {
        println!("skipped: CLUSTINE_STEELMC names no clone of SteelMC");
        return;
    };
    let path = std::path::Path::new(&clone).join("steel-core/test_assets/chunk_stage_hashes.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(fixture["seed"].as_i64(), Some(SEED));

    for (name, dimension, count) in [
        ("minecraft:overworld", Dimension::Overworld, 2500),
        ("minecraft:the_nether", Dimension::Nether, 2500),
    ] {
        let chunks = fixture["dimensions"][name]["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), count);
        let terrain = Terrain::new(SEED, dimension);
        let source = BiomeSource::new(SEED, dimension);
        let mut after_the_surface = 0;
        // Chunks that have the fixture's hash once they are carved: all of them, and
        // those that a carver changed.
        let mut equal = 0;
        let mut carved_and_equal = Vec::new();
        // Chunks that have not, with what stands around them.
        let mut different = Vec::new();
        let positions: Vec<(i32, i32)> = chunks
            .iter()
            .map(|entry| {
                (
                    entry["x"].as_i64().unwrap() as i32,
                    entry["z"].as_i64().unwrap() as i32,
                )
            })
            .collect();
        let mut holds = Vec::new();
        for (entry, (x, z)) in chunks.iter().zip(&positions) {
            let theirs = entry["stages"]["minecraft:terrain"].as_str().unwrap();
            let biomes = Biomes::from_source(&source, *x, *z);
            let surface = terrain.surface(*x, *z, &biomes);
            let carved = terrain.carved(*x, *z, &biomes, &source);
            let surfaced = hash(dimension, &surface) == theirs;
            after_the_surface += usize::from(surfaced);
            let held = hash(dimension, &carved.chunk) == theirs;
            holds.push(held);
            if held {
                equal += 1;
                if carved.chunk != surface {
                    carved_and_equal.push((*x, *z, theirs));
                }
            } else {
                different.push((*x, *z));
            }
        }
        // The chunks that have not the hash, as a list for
        // `tests/java/GameCarvers.java`: the game's own fill, surface and carvers
        // without structures then say whether these steps are what differs.
        if let Ok(directory) = std::env::var("CLUSTINE_STEELMC_DIFFERENT") {
            std::fs::create_dir_all(&directory).unwrap();
            let list: String = different
                .iter()
                .map(|(x, z)| format!("{x} {z}\n"))
                .collect();
            let file = format!("{}_{SEED}.txt", name.trim_start_matches("minecraft:the_"));
            let file = file.trim_start_matches("minecraft:");
            std::fs::write(std::path::Path::new(&directory).join(file), list).unwrap();
        }
        // The fixture's chunks lie in squares. A structure bends the ground of
        // chunks that lie together; a fault of the carvers would not keep to such
        // places. So for each chunk that has not the hash: how many of the eight
        // around it are of the fixture, and how many of those have not the hash
        // either.
        let mut alone = Vec::new();
        let mut neighbours = (0, 0);
        for (x, z) in &different {
            let mut around = 0;
            let mut around_different = 0;
            for (place, (other_x, other_z)) in positions.iter().enumerate() {
                if (other_x, other_z) != (x, z)
                    && (other_x - x).abs() <= 1
                    && (other_z - z).abs() <= 1
                {
                    around += 1;
                    around_different += usize::from(!holds[place]);
                }
            }
            if around_different == 0 {
                // Whether a cell the chunk's surface reads is an exact tie between
                // two biomes, which this crate does not always decide as the game.
                let (lowest, sections) = dimension.sections();
                let tied = (x * 4 - 1..=x * 4 + 4).any(|cell_x| {
                    (z * 4 - 1..=z * 4 + 4).any(|cell_z| {
                        (lowest * 4..(lowest + sections as i32) * 4)
                            .any(|cell_y| !source.found(cell_x, cell_y, cell_z).tied.is_empty())
                    })
                });
                alone.push((*x, *z, around, tied));
            }
            neighbours.0 += around;
            neighbours.1 += around_different;
        }
        println!(
            "{name}: {count} chunks; {equal} have the fixture's hash after the carvers \
             ({after_the_surface} had it after the surface), {} of them chunks a carver \
             changed, the first of those {:?}. {} have not: of the {} chunks of the \
             fixture that lie around one of them {} have not either, and {} of them \
             have no such neighbour (each with the number of the fixture's chunks around \
             it and whether a cell of its biomes is an exact tie): {alone:?}. They are \
             not judged: at `terrain` a chunk also has the ground that structures bent",
            carved_and_equal.len(),
            &carved_and_equal[..carved_and_equal.len().min(4)],
            different.len(),
            neighbours.0,
            neighbours.1,
            alone.len(),
        );
    }

    let chunks = fixture["dimensions"]["minecraft:the_end"]["chunks"]
        .as_array()
        .unwrap();
    let terrain = Terrain::new(SEED, Dimension::End);
    let mut equal = 0;
    let mut empty = 0;
    let mut different = Vec::new();
    for entry in chunks {
        let x = entry["x"].as_i64().unwrap() as i32;
        let z = entry["z"].as_i64().unwrap() as i32;
        let theirs = entry["stages"]["minecraft:terrain"].as_str().unwrap();
        let ours = hash(Dimension::End, &terrain.fill(x, z));
        empty += usize::from(theirs == EMPTY);
        if ours == theirs {
            equal += 1;
        } else {
            different.push((x, z, ours, theirs));
        }
    }
    println!(
        "minecraft:the_end: {} chunks; {equal} have the fixture's hash, {empty} of the \
         fixture's are empty; {} different",
        chunks.len(),
        different.len()
    );
    for (x, z, ours, theirs) in different.iter().take(20) {
        println!("  DIFFERENT chunk {x} {z}: here {ours}, the fixture {theirs}");
    }
    assert!(
        different.is_empty(),
        "chunks of the End differ from SteelMC's fixture"
    );
}
