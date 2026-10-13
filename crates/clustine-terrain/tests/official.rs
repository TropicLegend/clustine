//! The judge of the biomes: over chunks the official server made, every biome cell it
//! stored is the biome this crate gives, or one this crate finds exactly tied with it.
//!
//! Nothing of such a world is in the repository. The test takes the world's directory
//! from `CLUSTINE_SAMPLE_WORLD` and its seed from `CLUSTINE_SAMPLE_SEED` (13579 if that
//! is not set, the seed of the sample the terrain plan names), and reads the
//! overworld's region files itself, as `crates/clustine-light/tests/official.rs` does.
//!
//! ```text
//! CLUSTINE_SAMPLE_WORLD=<a world of the official server> \
//!     cargo test --release -p clustine-terrain -- --ignored --nocapture official
//! ```
//!
//! A cell that differs passes only as an exact tie: the game breaks a tie by what its
//! thread found before, this crate by the order of the game's tree alone
//! (`docs/groundwork/terrain-plan.md`, the rule of step T3). Such cells are counted
//! and the first of them printed, with the climate and both distances.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use clustine_data::synced_registry;
use clustine_terrain::biome::climate::PARAMETER_NAMES;
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_world::{BIOME_CELLS, Biome};

// --- NBT, as the region files have it --------------------------------------------------

#[derive(Debug)]
enum Nbt {
    Number(i64),
    Other,
    Text(String),
    List(Vec<Nbt>),
    Compound(BTreeMap<String, Nbt>),
    Longs(Vec<i64>),
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, count: usize) -> &[u8] {
        let taken = &self.bytes[self.at..self.at + count];
        self.at += count;
        taken
    }

    fn number(&mut self, bytes: usize) -> i64 {
        let mut value = 0i64;
        for (index, byte) in self.take(bytes).iter().enumerate() {
            // The first byte carries the sign.
            let byte = if index == 0 {
                i64::from(*byte as i8)
            } else {
                i64::from(*byte)
            };
            value = value << 8 | byte;
        }
        value
    }

    fn length(&mut self) -> usize {
        usize::try_from(self.number(4)).unwrap()
    }

    fn text(&mut self) -> String {
        let length = self.number(2) as usize;
        String::from_utf8_lossy(self.take(length)).into_owned()
    }

    fn skip(&mut self, bytes: usize) -> Nbt {
        self.take(bytes);
        Nbt::Other
    }

    fn payload(&mut self, kind: u8) -> Nbt {
        match kind {
            1 => Nbt::Number(self.number(1)),
            2 => Nbt::Number(self.number(2)),
            3 => Nbt::Number(self.number(4)),
            4 => Nbt::Number(self.number(8)),
            5 => self.skip(4),
            6 => self.skip(8),
            7 => {
                let length = self.length();
                self.skip(length)
            }
            8 => Nbt::Text(self.text()),
            9 => {
                let of = self.take(1)[0];
                let length = self.length();
                Nbt::List((0..length).map(|_| self.payload(of)).collect())
            }
            10 => {
                let mut compound = BTreeMap::new();
                loop {
                    let tag = self.take(1)[0];
                    if tag == 0 {
                        return Nbt::Compound(compound);
                    }
                    let name = self.text();
                    compound.insert(name, self.payload(tag));
                }
            }
            11 => {
                let length = self.length();
                self.skip(4 * length)
            }
            12 => {
                let length = self.length();
                Nbt::Longs((0..length).map(|_| self.number(8)).collect())
            }
            other => panic!("NBT tag {other}"),
        }
    }
}

impl Nbt {
    fn read(bytes: &[u8]) -> Nbt {
        let mut cursor = Cursor { bytes, at: 0 };
        let kind = cursor.take(1)[0];
        cursor.text();
        cursor.payload(kind)
    }

    fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Nbt::Compound(compound) => compound.get(key),
            _ => None,
        }
    }

    fn number(&self) -> i64 {
        match self {
            Nbt::Number(number) => *number,
            other => panic!("a number, not {other:?}"),
        }
    }

    fn text(&self) -> &str {
        match self {
            Nbt::Text(text) => text,
            other => panic!("a string, not {other:?}"),
        }
    }

    fn list(&self) -> &[Nbt] {
        match self {
            Nbt::List(list) => list,
            other => panic!("a list, not {other:?}"),
        }
    }
}

// --- A chunk of the official server ----------------------------------------------------

struct Official {
    status: String,
    /// By section coordinate: the 64 cells in the order of `y << 4 | z << 2 | x`,
    /// which is the order the game stores them in.
    biomes: BTreeMap<i64, Vec<Biome>>,
}

fn biome_named(name: &str) -> Biome {
    let biomes = synced_registry("minecraft:worldgen/biome").unwrap();
    let id = biomes
        .id_of(name)
        .unwrap_or_else(|| panic!("no biome {name}"));
    Biome(id as u16)
}

fn name_of(biome: Biome) -> &'static str {
    let biomes = synced_registry("minecraft:worldgen/biome").unwrap();
    biomes.entries[usize::from(biome.0)].trim_start_matches("minecraft:")
}

fn official(chunk: &Nbt) -> Official {
    let mut read = Official {
        status: chunk
            .get("Status")
            .map_or(String::new(), |status| status.text().to_owned()),
        biomes: BTreeMap::new(),
    };
    for section in chunk.get("sections").map_or(&[][..], Nbt::list) {
        let Some(biomes) = section.get("biomes") else {
            continue;
        };
        // An entry is the biome's name; a list that mixes kinds would wrap each in a
        // compound, as the palettes of blocks do.
        let palette: Vec<Biome> = biomes
            .get("palette")
            .unwrap()
            .list()
            .iter()
            .map(|entry| biome_named(entry.get("").unwrap_or(entry).text()))
            .collect();
        let cells = match biomes.get("data") {
            Some(Nbt::Longs(words)) => {
                // As many bits as the palette needs, and no index across two words.
                let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(1) as usize;
                let in_a_word = 64 / bits;
                let mask = (1u64 << bits) - 1;
                (0..BIOME_CELLS)
                    .map(|at| {
                        let word = words[at / in_a_word] as u64;
                        palette[(word >> (at % in_a_word * bits) & mask) as usize]
                    })
                    .collect()
            }
            _ => vec![palette[0]; BIOME_CELLS],
        };
        read.biomes
            .insert(section.get("Y").unwrap().number(), cells);
    }
    read
}

/// Every chunk of the overworld's region files, by position.
fn chunks_of(world: &Path) -> BTreeMap<(i32, i32), Official> {
    let regions = world.join("dimensions/minecraft/overworld/region");
    let mut names: Vec<_> = std::fs::read_dir(&regions)
        .unwrap_or_else(|error| panic!("{}: {error}", regions.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mca"))
        .collect();
    names.sort();
    let mut found = BTreeMap::new();
    for name in names {
        let data = std::fs::read(&name).unwrap();
        if data.len() < 8192 {
            continue;
        }
        for index in 0..1024 {
            let entry = u32::from_be_bytes(data[index * 4..index * 4 + 4].try_into().unwrap());
            let (offset, sectors) = ((entry >> 8) as usize, entry & 0xff);
            if offset == 0 || sectors == 0 {
                continue;
            }
            let start = offset * 4096;
            let length = u32::from_be_bytes(data[start..start + 4].try_into().unwrap()) as usize;
            let stored = &data[start + 5..start + 4 + length];
            let mut body = Vec::new();
            match data[start + 4] {
                1 => flate2::read::GzDecoder::new(stored)
                    .read_to_end(&mut body)
                    .unwrap(),
                2 => flate2::read::ZlibDecoder::new(stored)
                    .read_to_end(&mut body)
                    .unwrap(),
                3 => {
                    body.extend_from_slice(stored);
                    body.len()
                }
                other => panic!("compression {other} in {}", name.display()),
            };
            let chunk = Nbt::read(&body);
            let position = (
                chunk.get("xPos").unwrap().number() as i32,
                chunk.get("zPos").unwrap().number() as i32,
            );
            found.insert(position, official(&chunk));
        }
    }
    found
}

/// The statuses at which a chunk has no biomes of its own yet: the game makes them in
/// the step after the last of these.
const BEFORE_BIOMES: [&str; 3] = [
    "minecraft:empty",
    "minecraft:structure_starts",
    "minecraft:structure_references",
];

/// What the comparison of one set of chunks found.
#[derive(Default)]
struct Tally {
    chunks: usize,
    cells: usize,
    equal: usize,
    /// Cells where the official biome is another than this crate's and is exactly as
    /// near.
    ties: usize,
    /// Of those, the cells where the game's search gives the official biome if it
    /// remembers the cell before in the game's order of filling a chunk.
    ties_the_memory_explains: usize,
    differences: usize,
    shown: Vec<String>,
}

#[test]
#[ignore = "needs the sample world of the official server"]
#[allow(
    clippy::disallowed_methods,
    reason = "the test, not the crate, takes the time"
)]
fn official_chunks_have_the_biomes_this_crate_gives() {
    // Without a sample world there is nothing to compare with, and that is not to
    // fail the other comparisons this one is run with.
    let Ok(world) = std::env::var("CLUSTINE_SAMPLE_WORLD") else {
        println!("skipped: CLUSTINE_SAMPLE_WORLD names no world directory of the official server");
        return;
    };
    let seed: i64 = std::env::var("CLUSTINE_SAMPLE_SEED")
        .map_or(13579, |seed| seed.parse().expect("the seed is a number"));
    for dimension in ["the_nether", "the_end"] {
        let regions = Path::new(&world).join(format!("dimensions/minecraft/{dimension}/region"));
        if !regions.is_dir() {
            println!("the sample has no chunks of {dimension}: nothing of it is compared here");
        }
    }

    let chunks = chunks_of(Path::new(&world));
    let source = BiomeSource::new(seed, Dimension::Overworld);
    let tree = source.parameters().unwrap();
    let (lowest, section_count) = Dimension::Overworld.sections();

    // Finished chunks are what must agree; chunks the server left at an earlier
    // status have their biomes too, if they came as far, and are counted apart.
    let mut finished = Tally::default();
    let mut unfinished = Tally::default();
    let mut without = 0;
    let mut spent = std::time::Duration::ZERO;
    for (&(chunk_x, chunk_z), chunk) in &chunks {
        if BEFORE_BIOMES.contains(&chunk.status.as_str()) {
            without += 1;
            continue;
        }
        let tally = if chunk.status == "minecraft:full" {
            &mut finished
        } else {
            &mut unfinished
        };
        tally.chunks += 1;
        assert_eq!(
            chunk.biomes.len(),
            section_count,
            "the sections of the chunk {chunk_x}, {chunk_z}"
        );

        let started = std::time::Instant::now();
        let made = source.chunk(chunk_x, chunk_z);
        spent += started.elapsed();

        // The game's own search over the chunk, in the order the game fills it,
        // remembering the entry it found for the cell before and nothing at the
        // chunk's first cell. Only looked at where a cell differs.
        let mut replayed: Option<Vec<Vec<Biome>>> = None;

        for (index, cells) in made.iter().enumerate() {
            let section = i64::from(lowest) + index as i64;
            let stored = &chunk.biomes[&section];
            for cell in 0..BIOME_CELLS {
                tally.cells += 1;
                if cells[cell] == stored[cell] {
                    tally.equal += 1;
                    continue;
                }
                let x = chunk_x * 4 + (cell & 3) as i32;
                let y = section as i32 * 4 + (cell >> 4) as i32;
                let z = chunk_z * 4 + (cell >> 2 & 3) as i32;
                let found = source.found(x, y, z);
                assert_eq!(found.biome, cells[cell]);
                let nearest = found.nearest.unwrap();
                let theirs = tree.distance_to(stored[cell], &found.climate);
                let tied = found.tied.contains(&stored[cell]);
                assert_eq!(tied, theirs == Some(nearest.distance));
                if tied {
                    tally.ties += 1;
                    let replayed = replayed
                        .get_or_insert_with(|| with_the_games_memory(&source, chunk_x, chunk_z));
                    if replayed[index][cell] == stored[cell] {
                        tally.ties_the_memory_explains += 1;
                    }
                } else {
                    tally.differences += 1;
                }
                if tally.shown.len() < 20 {
                    let climate: Vec<String> = PARAMETER_NAMES
                        .iter()
                        .zip(found.climate.0)
                        .map(|(name, value)| format!("{name} {value}"))
                        .collect();
                    tally.shown.push(format!(
                        "  {} chunk {chunk_x}, {chunk_z} cell {x}, {y}, {z} (blocks {}, {}, {}): \
                         here {} at distance {}, the server {} at distance {}; {}",
                        if tied { "tie" } else { "DIFFERENT" },
                        x * 4,
                        y * 4,
                        z * 4,
                        name_of(cells[cell]),
                        nearest.distance,
                        name_of(stored[cell]),
                        theirs.map_or("none: not in the list".to_owned(), |d| d.to_string()),
                        climate.join(", "),
                    ));
                }
            }
        }
    }

    println!(
        "{} chunks read: {} finished, {} unfinished with biomes, {without} from before the \
         biomes step",
        chunks.len(),
        finished.chunks,
        unfinished.chunks,
    );
    for (name, tally) in [("finished", &finished), ("unfinished", &unfinished)] {
        println!(
            "{name}: {} chunks, {} cells: {} equal, {} exact ties ({} of them as the game's \
             memory within the chunk gives them), {} different",
            tally.chunks,
            tally.cells,
            tally.equal,
            tally.ties,
            tally.ties_the_memory_explains,
            tally.differences,
        );
        for line in &tally.shown {
            println!("{line}");
        }
    }
    let made = finished.chunks + unfinished.chunks;
    if made > 0 {
        println!(
            "{:.3} ms a chunk to make the biomes ({made} chunks, one thread, this build)",
            spent.as_secs_f64() * 1000.0 / made as f64
        );
    }

    assert!(finished.chunks > 0, "the sample has no finished chunk");
    assert_eq!(
        finished.equal + finished.ties + finished.differences,
        finished.cells
    );
    assert_eq!(
        (finished.differences, unfinished.differences),
        (0, 0),
        "cells differ that are no exact tie"
    );
}

/// The biomes of a chunk as the game's search gives them when it fills the chunk by
/// itself: section by section from the lowest, in each x, then y, then z, each search
/// starting from the entry found for the cell before.
fn with_the_games_memory(source: &BiomeSource, chunk_x: i32, chunk_z: i32) -> Vec<Vec<Biome>> {
    let tree = source.parameters().unwrap();
    let (lowest, section_count) = source.dimension().sections();
    let mut before = None;
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
