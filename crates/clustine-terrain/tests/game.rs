//! The biomes of chunks against the game's own classes: what `tests/java/GameBiomes.java`
//! wrote for the three dimensions, cell by cell.
//!
//! That program does the biomes step of the game's generator for a list of chunks, in
//! one thread and in the list's order, with the classes of the server jar and no
//! server. Nothing it writes is in the repository; this test takes the directory with
//! `overworld.txt`, `nether.txt` and `end.txt` from `CLUSTINE_GAME_BIOMES`.
//!
//! ```text
//! # With <classes> the jars `cargo datagen` unpacked (target/datagen/26.3/extract/classpath,
//! # every .jar under it), for each of overworld, nether and end, and a list of chunks
//! # with `x z` on a line:
//! javac -proc:none -cp <classes> -d <out> crates/clustine-terrain/tests/java/GameBiomes.java
//! java -cp <out>:<classes> GameBiomes 13579 overworld <list> <directory>/overworld.txt
//!
//! CLUSTINE_GAME_BIOMES=<directory> \
//!     cargo test --release -p clustine-terrain -- --ignored --nocapture game
//! ```
//!
//! Two things are held to the file, and the first is the stricter:
//!
//! - **The game's search, replayed.** The game's search of a parameter list starts
//!   from what its thread found last. The program asks in a known order, so the same
//!   searches in the same order, each handed the entry found before
//!   (`ParameterTree::nearest_after`), must give the game's biome at **every** cell,
//!   tied or not. That holds the climate, the order of the tree and the rule of the
//!   memory to the game all at once.
//! - **This crate's biomes**, which are a function of the position: every cell is the
//!   game's, or the two biomes are exactly tied there. The ties are counted.

use std::collections::BTreeMap;
use std::path::Path;

use clustine_data::synced_registry;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_world::{BIOME_CELLS, Biome};

/// The sections of a chunk, each its 64 cells in the order of `y << 4 | z << 2 | x`.
type Sections = Vec<Vec<Biome>>;

/// What the program wrote for one dimension.
struct Written {
    seed: i64,
    /// The chunks in the order they were asked for, each with its position.
    chunks: Vec<((i32, i32), Sections)>,
    /// The cells whose climate as a volume was not the climate at the cell alone.
    climates_differing: usize,
}

fn read(path: &Path, dimension: Dimension, name: &str) -> Written {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let registry = synced_registry("minecraft:worldgen/biome").unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("game-biomes 1"));
    let seed = lines.next().unwrap().strip_prefix("seed ").unwrap();
    assert_eq!(lines.next().unwrap().strip_prefix("dimension "), Some(name));
    let (lowest, count) = dimension.sections();
    assert_eq!(
        lines.next(),
        Some(format!("sections {lowest} {count}").as_str())
    );

    let mut biomes: BTreeMap<&str, Biome> = BTreeMap::new();
    let mut written = Written {
        seed: seed.parse().unwrap(),
        chunks: Vec::new(),
        climates_differing: 0,
    };
    for line in lines {
        let fields: Vec<&str> = line.split(' ').collect();
        match fields[..] {
            ["biome", number, name] => {
                let id = registry
                    .id_of(name)
                    .unwrap_or_else(|| panic!("no biome {name}"));
                biomes.insert(number, Biome(id as u16));
            }
            ["chunk", x, z, ref sections @ ..] => {
                assert_eq!(sections.len(), count);
                let sections = sections
                    .iter()
                    .map(|section| {
                        let cells: Vec<Biome> =
                            section.split(',').map(|number| biomes[number]).collect();
                        match cells.len() {
                            1 => vec![cells[0]; BIOME_CELLS],
                            BIOME_CELLS => cells,
                            other => panic!("a section of {other} cells"),
                        }
                    })
                    .collect();
                written
                    .chunks
                    .push(((x.parse().unwrap(), z.parse().unwrap()), sections));
            }
            ["differs", ..] => written.climates_differing += 1,
            ["cells", _, "differing", differing] => {
                assert_eq!(differing.parse(), Ok(written.climates_differing));
            }
            _ => panic!("the line {line:?} is not of the program's"),
        }
    }
    written
}

fn name_of(biome: Biome) -> &'static str {
    synced_registry("minecraft:worldgen/biome").unwrap().entries[usize::from(biome.0)]
}

#[test]
#[ignore = "needs what tests/java/GameBiomes.java writes from the server jar"]
fn chunks_have_the_biomes_the_games_own_classes_give() {
    let Ok(directory) = std::env::var("CLUSTINE_GAME_BIOMES") else {
        println!("skipped: CLUSTINE_GAME_BIOMES names no directory with the game's biomes");
        return;
    };
    let mut failed = false;
    for (dimension, name) in [
        (Dimension::Overworld, "overworld"),
        (Dimension::Nether, "nether"),
        (Dimension::End, "end"),
    ] {
        let path = Path::new(&directory).join(format!("{name}.txt"));
        let written = read(&path, dimension, name);
        let source = BiomeSource::new(written.seed, dimension);
        let (lowest, _) = dimension.sections();

        let mut cells = 0;
        // The replay of the game's search.
        let mut replay_differs = Vec::new();
        let mut before = None;
        // This crate's biomes.
        let (mut equal, mut ties) = (0, 0);
        let mut differs = Vec::new();
        let mut chunks_with_ties = 0;
        let mut kinds: BTreeMap<Biome, usize> = BTreeMap::new();

        for ((chunk_x, chunk_z), sections) in &written.chunks {
            let position = |index: usize, cell: usize| {
                (
                    chunk_x * 4 + (cell & 3) as i32,
                    (lowest + index as i32) * 4 + (cell >> 4) as i32,
                    chunk_z * 4 + (cell >> 2 & 3) as i32,
                )
            };

            if let Some(tree) = source.parameters() {
                for (index, theirs) in sections.iter().enumerate() {
                    for x in 0..4 {
                        for y in 0..4 {
                            for z in 0..4 {
                                let cell = y << 4 | z << 2 | x;
                                let (cell_x, cell_y, cell_z) = position(index, cell);
                                let climate = source.climate(cell_x, cell_y, cell_z);
                                let found = tree.nearest_after(&climate, before);
                                before = Some(found.entry);
                                if found.biome != theirs[cell] {
                                    replay_differs.push((cell_x, cell_y, cell_z));
                                }
                            }
                        }
                    }
                }
            }

            let made = source.chunk(*chunk_x, *chunk_z);
            let mut tied_here = false;
            for (index, (ours, theirs)) in made.iter().zip(sections).enumerate() {
                for cell in 0..BIOME_CELLS {
                    cells += 1;
                    *kinds.entry(theirs[cell]).or_default() += 1;
                    if ours[cell] == theirs[cell] {
                        equal += 1;
                        continue;
                    }
                    let (x, y, z) = position(index, cell);
                    if source.found(x, y, z).tied.contains(&theirs[cell]) {
                        ties += 1;
                        tied_here = true;
                    } else {
                        differs.push((x, y, z, ours[cell], theirs[cell]));
                    }
                }
            }
            chunks_with_ties += usize::from(tied_here);
        }

        println!(
            "{name}, seed {}: {} chunks, {cells} cells of {} biomes; the climate as a volume \
             differed from the climate of the cell alone at {} cells",
            written.seed,
            written.chunks.len(),
            kinds.len(),
            written.climates_differing,
        );
        if source.parameters().is_some() {
            println!(
                "  the game's search replayed: {} cells differ",
                replay_differs.len()
            );
        }
        println!(
            "  as this crate makes them: {equal} equal, {ties} exact ties in {chunks_with_ties} \
             chunks, {} different",
            differs.len()
        );
        for (x, y, z) in replay_differs.iter().take(10) {
            println!("  the replay differs at the cell {x}, {y}, {z}");
        }
        for (x, y, z, ours, theirs) in differs.iter().take(20) {
            let found = source.found(*x, *y, *z);
            println!(
                "  DIFFERENT at the cell {x}, {y}, {z}: here {}, the game {}; climate {:?}, \
                 nearest {:?}",
                name_of(*ours),
                name_of(*theirs),
                found.climate.0,
                found.nearest,
            );
        }
        failed |= !replay_differs.is_empty() || !differs.is_empty();
        // The comparison leans on the climate being the same either way; see the
        // head of `biome::climate`.
        failed |= written.climates_differing != 0;
    }
    assert!(
        !failed,
        "biomes differ from the game's by more than exact ties"
    );
}
