//! The fill against hashes SteelMC's extractor took from the game: for seed 13579, the
//! MD5 of a chunk's blocks at the game's `terrain` status.
//!
//! The hashes are SteelMC's (`steel-core/test_assets/chunk_stage_hashes.json` at
//! 885c4b3; see `NOTICE` in this crate), and the way of hashing is that of its
//! `steel-core/src/worldgen/chunk_stage_hashes.rs`. A few are copied into this file
//! and compared in every run. All 2,500 chunks of the End are compared where a clone
//! of SteelMC is named, for its fixtures are read in the clone and not committed
//! (ADR-0019, section 2):
//!
//! ```text
//! CLUSTINE_STEELMC=<a clone of SteelMC at 885c4b3> \
//!     cargo test --release -p clustine-terrain -- --ignored --nocapture steelmc
//! ```
//!
//! **Only the End can be compared yet, and of the End what the jar gives.** A chunk
//! at the `terrain` status has been filled, has had its surface made by the
//! dimension's material rule, and has been carved; and structures near it have bent
//! its ground. The fill is the first of those alone:
//!
//! - *The End*: its material rule puts end stone where end stone is, no biome of it
//!   has a carver, and no structure of it bends the ground. So a chunk of the End at
//!   `terrain` is the chunk after the fill, and the fixture's hash is of the fill.
//! - *The overworld and the Nether*: bedrock, the surface and the ore veins (step T5
//!   of `docs/groundwork/terrain-plan.md`), the carvers (T6) and the structures that
//!   bend the ground (T7b) come first. Their hashes are set aside until then, all
//!   2,500 of each.
//!
//! **What it gave**, on 2026-10-13: all 2,500 chunks of the End have the fixture's
//! hash; 1,008 of them are empty. That includes the 1,300 chunks far out for which the
//! same fixture's *biomes* are not what the 26.3 jar gives (`tests/steelmc_hashes.rs`
//! says why): the blocks of its `terrain` status are the jar's there too.

use clustine_data::BlockState;
use clustine_terrain::biome::Dimension;
use clustine_terrain::fill::{Filled, Terrain};
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

    for (name, count) in [
        ("minecraft:overworld", 2500),
        ("minecraft:the_nether", 2500),
    ] {
        let chunks = fixture["dimensions"][name]["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), count);
        println!(
            "{name}: {count} chunks set aside: at `terrain` they have a surface, caves and \
             the ground structures bent, which the fill has not"
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
