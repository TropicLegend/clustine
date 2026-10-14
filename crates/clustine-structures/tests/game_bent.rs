//! The ground that structures bend against the game's own classes, block by block,
//! and against the hashes SteelMC's extractor took from the game.
//!
//! `crates/clustine-terrain/tests/java/GameStructures.java`, for a line `terrain <x>
//! <z>` of its list, has the pinned jar's generator make the starts and references
//! around a chunk and then its fill, its surface and its carvers **with the
//! beardifier of those structures**, without a server (its head says how). This
//! test takes a directory from `CLUSTINE_GAME_BENT` and reads every `.txt` file in
//! it; nothing of them is in the repository.
//!
//! ```text
//! java -cp <out>:<classes> GameStructures <seed> <dimension> <list> <directory>/<name>.txt
//! CLUSTINE_GAME_BENT=<directory> \
//!     cargo test --release -p clustine-structures --test game_bent -- --ignored --nocapture bent_as
//! ```
//!
//! **The judge** ([`chunks_are_bent_as_the_games_own_classes_bend_them`]): for every
//! chunk the beardifier's pieces and junctions equal the game's (in the game's
//! order, or else as the same pieces in another order, which is counted: see
//! `Structures::beardifier`), the box it bends in equal, and every block of fill,
//! surface and carvers equal, made with the starts of this crate, the biomes the
//! game stored around the chunk for the surface and this crate's own biomes for the
//! carvers. The kernel of the beardifier is held to the game's by a hash. A block that
//! differs beside a cell where this crate finds two biomes exactly tied is counted
//! apart and not judged: what a carver lays bare is made with a biome the game asks
//! its source for anew, and at a tie the game's answer is not a function of the
//! position (`clustine_terrain::biome`).
//!
//! **What was run**, on 2026-10-14 on the owner's machine (release build), and what
//! it gave. The chunks: for every start of a structure that adapts the terrain in
//! the lists of `tests/game_structures.rs` six chunks spread over its box (four on
//! the two smaller seeds), and the 326 chunks of the gate below. **9,875 chunks**
//! (9,224 of the overworld, 651 of the Nether) on three seeds, 8,882 of them with a
//! beardifier: **every one of 928,088,064 blocks equal but two**, which lie beside
//! an exact tie of biomes (podzol for grass, in one chunk); the structures changed
//! 3,248,404 blocks of them. Every beardifier has the game's pieces and junctions
//! and bends in the game's box; in 321 chunks, where two structures bend one chunk
//! (a camp over a trial chamber), the pieces come in another order than in that run
//! of the game, and no block differs for it. The kernel is the game's.
//!
//! **That the judge can fail** was tried on 422 chunks of seed 0: a thin beard
//! weighed 0.75 in place of 0.8 (935 blocks in 130 chunks), a junction weighed 0.5
//! in place of 0.4 (646 blocks), an encapsulated piece not weighed 0.8 (52 blocks).
//! Making the kernel with `exp` in place of the game's `Math.pow` of e changes none
//! of its 13,824 numbers.
//!
//! **The gate of SteelMC's hashes**
//! ([`chunks_of_steelmcs_fixture_have_their_hashes_with_structures_on`]): the
//! `terrain` hashes of its fixture of seed 13579 (2,500 chunks each of the overworld
//! and the Nether), which without structures 56 and 270 chunks did not have
//! (`crates/clustine-terrain/tests/steelmc_terrain.rs`). Where
//! `CLUSTINE_STEELMC_BENT_LISTS` names a directory, the chunks that do not have the
//! hash without structures are written there as lists for the program. **With
//! structures on, 2,497 of the overworld's 2,500 chunks and 2,498 of the Nether's
//! have the fixture's hash: 321 of the 326.** The five others have no structure
//! near; they and the 321 are equal to the game's own chunks block for block where
//! the game's biomes are given (above), so what is left in them is a biome that is
//! an exact tie.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clustine_data::BlockState;
use clustine_structures::{Start, Structures};
use clustine_terrain::beard::{Adjustment, Beardifier};
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::Filled;
use clustine_terrain::surface::Biomes;
use clustine_world::Biome;
use md5::{Digest, Md5};

fn dimension_of(name: &str) -> Dimension {
    match name {
        "overworld" => Dimension::Overworld,
        "nether" => Dimension::Nether,
        "end" => Dimension::End,
        other => panic!("no dimension {other}"),
    }
}

/// The starts of chunks, made when first asked for.
struct Starts<'a> {
    structures: &'a Structures,
    made: BTreeMap<(i32, i32), Vec<Start>>,
}

impl Starts<'_> {
    fn around(&mut self, chunk_x: i32, chunk_z: i32) {
        for x in chunk_x - 8..=chunk_x + 8 {
            for z in chunk_z - 8..=chunk_z + 8 {
                self.made
                    .entry((x, z))
                    .or_insert_with(|| self.structures.starts(x, z));
            }
        }
    }

    fn beardifier(&mut self, chunk_x: i32, chunk_z: i32) -> Beardifier {
        self.around(chunk_x, chunk_z);
        Structures::beardifier(chunk_x, chunk_z, |x, z| self.made[&(x, z)].as_slice())
    }
}

/// The beardifier as the program writes it: the box, the pieces, the junctions.
fn beard_text(beard: &Beardifier) -> (String, Vec<String>, Vec<String>) {
    let bounds = match beard.affected() {
        None => "-".to_owned(),
        Some((min, max)) => {
            format!(
                "{},{},{},{},{},{}",
                min[0], min[1], min[2], max[0], max[1], max[2]
            )
        }
    };
    let rigids = beard
        .rigids()
        .iter()
        .map(|rigid| {
            format!(
                "{},{},{},{},{},{},{},{}",
                rigid.min[0],
                rigid.min[1],
                rigid.min[2],
                rigid.max[0],
                rigid.max[1],
                rigid.max[2],
                match rigid.adjustment {
                    Adjustment::Bury => "BURY",
                    Adjustment::BeardThin => "BEARD_THIN",
                    Adjustment::BeardBox => "BEARD_BOX",
                    Adjustment::Encapsulate => "ENCAPSULATE",
                },
                rigid.ground_level_delta
            )
        })
        .collect();
    // The program writes a junction with its change of height, which the sum does
    // not read; it is cut off there.
    let junctions = beard
        .junctions()
        .iter()
        .map(|junction| format!("{},{},{}", junction.x, junction.ground_y, junction.z))
        .collect();
    (bounds, rigids, junctions)
}

fn runs(fields: &[&str]) -> Vec<BlockState> {
    let mut blocks = Vec::new();
    for run in fields {
        let (state, count) = run.split_once('*').expect("a run is a state and a count");
        let state = BlockState(state.parse().expect("a state"));
        blocks.extend(std::iter::repeat_n(state, count.parse().expect("a count")));
    }
    blocks
}

fn fnv(bytes: impl Iterator<Item = u8>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[test]
#[ignore = "needs what GameStructures.java wrote for `terrain` lines, in CLUSTINE_GAME_BENT"]
fn chunks_are_bent_as_the_games_own_classes_bend_them() {
    let directory =
        PathBuf::from(std::env::var_os("CLUSTINE_GAME_BENT").expect("a directory of outputs"));
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .expect("the directory can be read")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no output of the program in {}",
        directory.display()
    );
    let our_kernel = fnv(clustine_terrain::beard::kernel()
        .iter()
        .flat_map(|value| value.to_bits().to_le_bytes()));

    let mut wrong = 0;
    for path in &paths {
        let text = std::fs::read_to_string(path).expect("the file can be read");
        let mut seed = 0;
        let mut dimension = Dimension::Overworld;
        let mut structures: Option<Structures> = None;
        let mut made = BTreeMap::new();
        let mut chunk = (0, 0);
        let mut their_beard: Option<(String, Vec<String>, Vec<String>)> = None;
        let mut columns: Vec<Vec<Biome>> = Vec::new();
        let (mut chunks, mut equal, mut bent, mut reordered, mut beards_wrong) = (0, 0, 0, 0, 0);
        let (mut blocks_wrong, mut blocks_bent, mut blocks_total) = (0_usize, 0_usize, 0_usize);
        let mut blocks_tied = 0_usize;
        let mut first: Vec<String> = Vec::new();
        for line in text.lines() {
            let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
            match word {
                "seed" => seed = rest.parse().expect("a seed"),
                "dimension" => dimension = dimension_of(rest),
                "kernel" => {
                    let theirs = u64::from_str_radix(rest, 16).expect("a hash");
                    assert_eq!(
                        our_kernel, theirs,
                        "the beardifier's kernel is not the game's"
                    );
                }
                "chunk" => {
                    let (x, z) = rest.split_once(' ').expect("x and z");
                    chunk = (x.parse().expect("x"), z.parse().expect("z"));
                }
                "beard" => {
                    let parts: Vec<&str> = rest.split(" |").collect();
                    let list = |text: &str, fields: usize| -> Vec<String> {
                        text.split_whitespace()
                            .map(|item| item.split(',').take(fields).collect::<Vec<_>>().join(","))
                            .collect()
                    };
                    their_beard = Some((
                        parts[0].trim().to_owned(),
                        list(parts[1], 8),
                        list(parts[2], 3),
                    ));
                }
                "biomes" => {
                    columns = rest
                        .split(' ')
                        .map(|column| {
                            let mut cells = Vec::new();
                            for run in column.split(',') {
                                let (of, count) = run.split_once('*').expect("a run of biomes");
                                cells.extend(std::iter::repeat_n(
                                    Biome(of.parse().expect("a biome")),
                                    count.parse().expect("a count"),
                                ));
                            }
                            cells
                        })
                        .collect();
                    assert_eq!(columns.len(), 36);
                }
                "blocks" => {
                    let structures =
                        structures.get_or_insert_with(|| Structures::new(seed, dimension));
                    let mut starts = Starts {
                        structures,
                        made: std::mem::take(&mut made),
                    };
                    let beard = starts.beardifier(chunk.0, chunk.1);
                    made = starts.made;
                    chunks += 1;
                    bent += usize::from(!beard.is_empty());

                    let ours = beard_text(&beard);
                    let theirs = their_beard.take().expect("a beardifier before the blocks");
                    if ours != theirs {
                        let sorted = |mut list: Vec<String>| {
                            list.sort();
                            list
                        };
                        if ours.0 == theirs.0
                            && sorted(ours.1.clone()) == sorted(theirs.1.clone())
                            && sorted(ours.2.clone()) == sorted(theirs.2.clone())
                        {
                            reordered += 1;
                            if reordered == 1
                                && std::env::var_os("CLUSTINE_GAME_BENT_ORDER").is_some()
                            {
                                println!(
                                    "  chunk {} {}: here {:?} and {:?}, the game {:?} and {:?}",
                                    chunk.0, chunk.1, ours.1, ours.2, theirs.1, theirs.2
                                );
                            }
                        } else {
                            beards_wrong += 1;
                            if first.len() < 6 {
                                first.push(format!(
                                    "chunk {} {}: the beardifier is {ours:?} for {theirs:?}",
                                    chunk.0, chunk.1
                                ));
                            }
                        }
                    }

                    let fields: Vec<&str> = rest.split(' ').collect();
                    let game = runs(&fields);
                    let mut given = columns.iter();
                    let biomes = Biomes::from_columns(dimension, chunk.0, chunk.1, |_, _| {
                        given.next().expect("36 columns").clone()
                    });
                    let terrain = structures.terrain();
                    let carved =
                        terrain.carved_bent(chunk.0, chunk.1, &beard, &biomes, structures.biomes());
                    let unbent = if beard.is_empty() {
                        None
                    } else {
                        Some(
                            terrain
                                .carved(chunk.0, chunk.1, &biomes, structures.biomes())
                                .chunk,
                        )
                    };
                    let settings = terrain.settings();
                    let mut different = 0;
                    let mut index = 0;
                    for z in 0..16 {
                        for x in 0..16 {
                            for y in settings.min_y..settings.min_y + settings.height {
                                let mine = carved.chunk.get(x, y, z);
                                if let Some(unbent) = &unbent {
                                    blocks_bent += usize::from(unbent.get(x, y, z) != game[index]);
                                }
                                if mine != game[index] {
                                    // What the carvers lay bare they ask the biome
                                    // source about anew, and where two biomes are
                                    // exactly tied the game's answer is not a
                                    // function of the position.
                                    let (cell_x, cell_z) = (
                                        (chunk.0 * 16 + x as i32) >> 2,
                                        (chunk.1 * 16 + z as i32) >> 2,
                                    );
                                    let tied = (cell_x - 1..=cell_x + 1).any(|cell_x| {
                                        (cell_z - 1..=cell_z + 1).any(|cell_z| {
                                            ((y >> 2) - 1..=(y >> 2) + 1).any(|cell_y| {
                                                !structures
                                                    .biomes()
                                                    .found(cell_x, cell_y, cell_z)
                                                    .tied
                                                    .is_empty()
                                            })
                                        })
                                    });
                                    if tied {
                                        blocks_tied += 1;
                                    } else {
                                        different += 1;
                                    }
                                    if first.len() < 6 {
                                        first.push(format!(
                                            "chunk {} {} block {x} {y} {z}: {} for {}{}; the \
                                             beardifier adds {}",
                                            chunk.0,
                                            chunk.1,
                                            mine.0,
                                            game[index].0,
                                            if tied {
                                                " beside an exact tie of biomes"
                                            } else {
                                                ""
                                            },
                                            beard.sample(
                                                chunk.0 * 16 + x as i32,
                                                y,
                                                chunk.1 * 16 + z as i32
                                            )
                                        ));
                                    }
                                }
                                index += 1;
                            }
                        }
                    }
                    blocks_total += index;
                    blocks_wrong += different;
                    equal += usize::from(different == 0);
                }
                _ => {}
            }
        }
        if chunks == 0 {
            continue;
        }
        println!(
            "{}: {chunks} chunks of the {dimension:?} of seed {seed}, {bent} of them with a \
             beardifier ({reordered} with the game's pieces in another order, {beards_wrong} \
             with other pieces); {equal} equal block for block; {blocks_wrong} of \
             {blocks_total} blocks differ, and {blocks_tied} more beside an exact tie of biomes, \
             which are not judged; the structures changed {blocks_bent} blocks",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        for line in &first {
            println!("  {line}");
        }
        wrong += blocks_wrong + beards_wrong;
    }
    assert_eq!(wrong, 0, "bent chunks differ from the game's");
}

/// SteelMC's hash of a chunk's blocks, as `crates/clustine-terrain/tests/steelmc_terrain.rs`
/// takes it.
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
#[ignore = "needs a clone of SteelMC, named by CLUSTINE_STEELMC"]
fn chunks_of_steelmcs_fixture_have_their_hashes_with_structures_on() {
    let clone = std::env::var("CLUSTINE_STEELMC").expect("CLUSTINE_STEELMC names a clone");
    let path = PathBuf::from(clone).join("steel-core/test_assets/chunk_stage_hashes.json");
    let text = std::fs::read_to_string(&path).expect("the fixture can be read");
    let fixture: serde_json::Value = serde_json::from_str(&text).expect("the fixture is JSON");
    let seed = fixture["seed"].as_i64().expect("a seed");
    let lists = std::env::var_os("CLUSTINE_STEELMC_BENT_LISTS").map(PathBuf::from);
    let mut still_different = 0;
    for (name, short, dimension) in [
        ("minecraft:overworld", "overworld", Dimension::Overworld),
        ("minecraft:the_nether", "nether", Dimension::Nether),
    ] {
        let chunks = fixture["dimensions"][name]["chunks"]
            .as_array()
            .expect("chunks");
        let structures = Structures::new(seed, dimension);
        let source = BiomeSource::new(seed, dimension);
        let mut starts = Starts {
            structures: &structures,
            made: BTreeMap::new(),
        };
        let (mut unbent_equal, mut bent_equal, mut with_beard) = (0, 0, 0);
        let mut gate = Vec::new();
        let mut different = Vec::new();
        for entry in chunks {
            let x = entry["x"].as_i64().expect("x") as i32;
            let z = entry["z"].as_i64().expect("z") as i32;
            let theirs = entry["stages"]["minecraft:terrain"]
                .as_str()
                .expect("a hash");
            let biomes = Biomes::from_source(&source, x, z);
            let terrain = structures.terrain();
            let unbent = terrain.carved(x, z, &biomes, &source).chunk;
            let held_unbent = hash(dimension, &unbent) == theirs;
            unbent_equal += usize::from(held_unbent);
            let beard = starts.beardifier(x, z);
            with_beard += usize::from(!beard.is_empty());
            let held = if beard.is_empty() {
                held_unbent
            } else {
                let bent = terrain.carved_bent(x, z, &beard, &biomes, &source).chunk;
                hash(dimension, &bent) == theirs
            };
            bent_equal += usize::from(held);
            if !held_unbent {
                gate.push((x, z, held));
            }
            if !held {
                different.push((x, z, !beard.is_empty()));
            }
            // The starts of chunks far behind are let go.
            if starts.made.len() > 40_000 {
                starts.made.clear();
            }
        }
        if let Some(lists) = &lists {
            std::fs::create_dir_all(lists).expect("the directory can be made");
            let list: String = gate
                .iter()
                .map(|(x, z, _)| format!("terrain {x} {z}\n"))
                .collect();
            std::fs::write(lists.join(format!("{short}_{seed}_gate.txt")), list)
                .expect("the list can be written");
        }
        println!(
            "{name}: {} chunks, {with_beard} with a beardifier; {unbent_equal} have the \
             fixture's hash without structures and {bent_equal} with them. Of the {} that \
             have it not without, {} have it with. Still different (x, z, whether a \
             beardifier is there): {different:?}",
            chunks.len(),
            gate.len(),
            gate.iter().filter(|(_, _, held)| *held).count(),
        );
        still_different += different.len();
    }
    // A chunk may hold a biome that is an exact tie, which the game does not decide
    // by the position (`clustine_terrain::biome`); those are printed above and are
    // judged block for block with the game's biomes by the test beside this one.
    println!("{still_different} chunks have not the fixture's hash");
}
