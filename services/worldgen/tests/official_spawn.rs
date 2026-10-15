//! The judges of a new world's spawn against worlds the official server made
//! (`docs/groundwork/t8-plan.md`, section 3, T8b).
//!
//! Judge one runs the spiral and the column rule over the official server's own
//! chunks, read from its region files, from the chunk its climate search ends in; the
//! result must be the spawn in its `level.dat` exactly. That judges the logic apart
//! from features, which the official chunks carry and the rule reads as they are.
//!
//! Judge two compares the spawn of the land with features (phase F), over Clustine's
//! own chunks made by the scheduler as a running server makes them, with the official
//! spawn. Since phase F places the game's features they are to be equal; where they
//! are not, what stands on the official column and not on Clustine's is printed, which
//! until phase S can be a structure's building.
//!
//! Nothing of such a world is in the repository. The directory holds one directory per
//! seed, each with the server's `world`; it is named by `CLUSTINE_SPAWN_WORLDS`:
//!
//! ```text
//! CLUSTINE_SPAWN_WORLDS=<directory> \
//!     cargo test --release -p clustine-worldgen --locked --test official_spawn -- --ignored --nocapture
//! ```
//!
//! 26.3 keeps a world's seed in `data/minecraft/world_gen_settings.dat`
//! (`data.seed`), not in `level.dat`, and the overworld's region files under
//! `dimensions/minecraft/overworld/region`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use clustine_data::BlockState;
use clustine_world::{Biome, BlockPos, Chunk, ChunkGenerator, ChunkPos};
use std::sync::Arc;

use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::spawn::{feet, place_to_stand, spiral};
use clustine_worldgen::{FeatureStages, TerrainGenerator};

// --- NBT, as the region files and level.dat have it ------------------------------------

#[derive(Debug)]
enum Nbt {
    Number(i64),
    Real,
    Bytes,
    Text(String),
    List(Vec<Nbt>),
    Compound(BTreeMap<String, Nbt>),
    Ints(Vec<i32>),
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
        let length = self.number(2) as u16 as usize;
        String::from_utf8_lossy(self.take(length)).into_owned()
    }

    fn payload(&mut self, kind: u8) -> Nbt {
        match kind {
            1 => Nbt::Number(self.number(1)),
            2 => Nbt::Number(self.number(2)),
            3 => Nbt::Number(self.number(4)),
            4 => Nbt::Number(self.number(8)),
            5 => {
                self.take(4);
                Nbt::Real
            }
            6 => {
                self.take(8);
                Nbt::Real
            }
            7 => {
                let length = self.length();
                self.take(length);
                Nbt::Bytes
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
                Nbt::Ints((0..length).map(|_| self.number(4) as i32).collect())
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
    /// A root tag, whose name is skipped.
    fn read(bytes: &[u8]) -> Nbt {
        let mut cursor = Cursor { bytes, at: 0 };
        let kind = cursor.take(1)[0];
        cursor.text();
        cursor.payload(kind)
    }

    /// A gzipped file of NBT, as `level.dat` is.
    fn read_gzipped(path: &Path) -> Nbt {
        let data =
            std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let mut body = Vec::new();
        flate2::read::GzDecoder::new(&data[..])
            .read_to_end(&mut body)
            .unwrap();
        Nbt::read(&body)
    }

    fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Nbt::Compound(compound) => compound.get(key),
            _ => None,
        }
    }

    fn at(&self, keys: &[&str]) -> &Nbt {
        keys.iter().fold(self, |nbt, key| {
            nbt.get(key)
                .unwrap_or_else(|| panic!("no {key} in the path {keys:?}"))
        })
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

    fn ints(&self) -> &[i32] {
        match self {
            Nbt::Ints(ints) => ints,
            other => panic!("an int array, not {other:?}"),
        }
    }
}

// --- A chunk of the official server ----------------------------------------------------

/// The overworld's number of sections.
const SECTION_COUNT: usize = 24;

/// The state a palette entry names: a bare name for a block's default state, or a
/// compound of a name and properties.
fn palette_entry(entry: &Nbt) -> BlockState {
    let entry = entry.get("").unwrap_or(entry);
    let state = match entry {
        Nbt::Text(name) => BlockState::from_name_and_properties(name, []),
        compound => {
            let name = compound.get("id").or(compound.get("Name")).unwrap().text();
            let properties = compound.get("properties").or(compound.get("Properties"));
            let pairs: Vec<(&str, &str)> = match properties {
                Some(Nbt::Compound(properties)) => properties
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.text()))
                    .collect(),
                _ => Vec::new(),
            };
            BlockState::from_name_and_properties(name, pairs)
        }
    };
    state.unwrap_or_else(|| panic!("no block state for the palette entry {entry:?}"))
}

/// A chunk of the region files as a chunk of this server, whose heightmaps are worked
/// out from its blocks, and whether the official server had finished it.
fn official_chunk(chunk: &Nbt) -> (bool, Chunk) {
    let lowest = chunk.get("yPos").unwrap().number();
    let full = chunk
        .get("Status")
        .is_some_and(|status| status.text() == "minecraft:full");
    let air = clustine_data::blocks::AIR;
    let mut sections = vec![clustine_world::Section::filled(air, Biome(0)); SECTION_COUNT];
    for section in chunk.get("sections").map_or(&[][..], Nbt::list) {
        let y = section.get("Y").unwrap().number();
        let Ok(index) = usize::try_from(y - lowest) else {
            continue;
        };
        if index >= SECTION_COUNT {
            continue;
        }
        let Some(states) = section.get("block_states") else {
            continue;
        };
        let palette: Vec<BlockState> = states
            .get("palette")
            .unwrap()
            .list()
            .iter()
            .map(palette_entry)
            .collect();
        sections[index] = match states.get("data") {
            Some(Nbt::Longs(words)) => {
                let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(4) as usize;
                let in_a_word = 64 / bits;
                let mask = (1u64 << bits) - 1;
                let states: Vec<BlockState> = (0..4096)
                    .map(|at| {
                        let word = words[at / in_a_word] as u64;
                        palette[(word >> (at % in_a_word * bits) & mask) as usize]
                    })
                    .collect();
                clustine_world::Section::from_states(
                    states.into_boxed_slice().try_into().unwrap(),
                    Biome(0),
                )
            }
            _ => clustine_world::Section::filled(palette[0], Biome(0)),
        };
    }
    (full, Chunk::from_sections(lowest as i32 * 16, sections))
}

/// Every finished chunk of the overworld's region files, by position.
fn chunks_of(world: &Path) -> BTreeMap<ChunkPos, Chunk> {
    let regions = world.join("dimensions/minecraft/overworld/region");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&regions)
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
            let position = ChunkPos::new(
                chunk.get("xPos").unwrap().number() as i32,
                chunk.get("zPos").unwrap().number() as i32,
            );
            let (full, chunk) = official_chunk(&chunk);
            if full {
                found.insert(position, chunk);
            }
        }
    }
    found
}

// --- A world of the official server ----------------------------------------------------

struct World {
    seed: i64,
    spawn: BlockPos,
    chunks: BTreeMap<ChunkPos, Chunk>,
}

impl World {
    fn read(directory: &Path) -> World {
        let world = directory.join("world");
        let settings = Nbt::read_gzipped(&world.join("data/minecraft/world_gen_settings.dat"));
        let seed = settings.at(&["data", "seed"]).number();
        // The directory is named by the seed, which says the files are the ones meant.
        if let Some(name) = directory.file_name().and_then(|name| name.to_str()) {
            assert_eq!(
                name.parse::<i64>().ok(),
                Some(seed),
                "the world in {} has the seed {seed}",
                directory.display()
            );
        }
        let level = Nbt::read_gzipped(&world.join("level.dat"));
        let pos = level.at(&["Data", "spawn", "pos"]).ints();
        assert_eq!(pos.len(), 3, "Data.spawn.pos is {pos:?}");
        World {
            seed,
            spawn: BlockPos::new(pos[0], pos[1], pos[2]),
            chunks: chunks_of(&world),
        }
    }

    fn chunk(&self, position: ChunkPos) -> &Chunk {
        self.chunks.get(&position).unwrap_or_else(|| {
            panic!(
                "seed {}: the official world has no finished chunk {position:?}",
                self.seed
            )
        })
    }
}

/// The worlds of `CLUSTINE_SPAWN_WORLDS`, or `None` where it names none.
fn worlds() -> Option<Vec<PathBuf>> {
    let Ok(directory) = std::env::var("CLUSTINE_SPAWN_WORLDS") else {
        println!("skipped: CLUSTINE_SPAWN_WORLDS names no directory of official worlds");
        return None;
    };
    let mut found: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{directory}: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("world/level.dat").is_file())
        .collect();
    found.sort();
    assert!(
        !found.is_empty(),
        "no world with a level.dat in {directory}"
    );
    Some(found)
}

/// The chunk the spiral begins at for this seed.
fn origin(seed: i64) -> ChunkPos {
    let [x, z] = clustine_terrain::spawn::search_origin(seed);
    ChunkPos::new(x >> 4, z >> 4)
}

// --- What a feature looks like ---------------------------------------------------------

fn name(state: BlockState) -> &'static str {
    state.block().map_or("?", |block| block.name)
}

/// Whether the block is one that a feature puts down and the generator's terrain alone
/// does not (section 2: trees, ice from freezing, snow, leaves).
fn is_feature_block(state: BlockState) -> bool {
    let name = name(state);
    name.ends_with("_leaves")
        || name.ends_with("_log")
        || name.ends_with("_wood")
        || name.ends_with("_stem")
        || name.ends_with("_hyphae")
        || name.ends_with("mushroom_block")
        || name.ends_with("ice")
        || name.ends_with("snow")
        || name.ends_with("snow_block")
        || name.ends_with("vine")
}

/// What feature, if any, the official column shows at or above `from` that Clustine's
/// lacks: a feature's block where Clustine's column has another, or a fluid where
/// Clustine's column has none at that height (a lake or a spring) or none where it has
/// one. A block of a feature that the surface rules put down as well, such as snow on
/// a peak, is the same in both and explains nothing.
fn feature_on(official: &Chunk, clustine: &Chunk, x: usize, z: usize, from: i32) -> Option<String> {
    let top = official.min_y() + official.height() as i32;
    let from = from.max(official.min_y());
    for y in (from..top).rev() {
        let state = official.get(x, y, z).unwrap();
        if is_feature_block(state) && clustine.get(x, y, z) != Some(state) {
            return Some(format!("{} at y {y}", name(state)));
        }
    }
    for y in (from..top).rev() {
        let theirs = official.get(x, y, z).unwrap().props().fluid().is_some();
        let ours = clustine.get(x, y, z).unwrap().props().fluid().is_some();
        if theirs != ours {
            return Some(format!(
                "a fluid {} at y {y}",
                if theirs { "added" } else { "taken" }
            ));
        }
    }
    None
}

/// The columns in the order the spawn's search tries them, as positions in the world.
fn columns_in_order(origin: ChunkPos) -> impl Iterator<Item = (ChunkPos, usize, usize)> {
    spiral().flat_map(move |(dx, dz)| {
        let chunk = ChunkPos::new(origin.x + dx, origin.z + dz);
        (0..16).flat_map(move |x| (0..16).map(move |z| (chunk, x, z)))
    })
}

fn column_of(position: BlockPos) -> (ChunkPos, usize, usize) {
    let (x, z) = position.in_chunk();
    (position.chunk(), x, z)
}

// --- The judges -------------------------------------------------------------------------

#[test]
#[ignore = "needs worlds the official server made"]
fn the_spiral_and_column_rule_over_the_official_chunks_give_the_official_spawn() {
    let Some(worlds) = worlds() else {
        return;
    };
    let mut wrong = Vec::new();
    for directory in &worlds {
        let world = World::read(directory);
        let origin = origin(world.seed);
        let found = place_to_stand(origin, |position| world.chunk(position));
        let verdict = if found == world.spawn {
            "equal"
        } else {
            wrong.push(world.seed);
            "DIFFERENT"
        };
        println!(
            "judge one, seed {}: origin chunk {}, {}; official {:?}; over its chunks {:?}: {verdict}",
            world.seed, origin.x, origin.z, world.spawn, found
        );
    }
    assert!(
        wrong.is_empty(),
        "the rule over the official chunks missed the official spawn for seeds {wrong:?}"
    );
}

#[test]
#[ignore = "needs worlds the official server made"]
fn the_generators_spawn_is_the_official_one_where_no_feature_explains_otherwise() {
    let Some(worlds) = worlds() else {
        return;
    };
    let templates = Arc::new(
        clustine_worldgen::templates::read_jar(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/datagen/26.3/server.jar"),
        )
        .expect("the templates of the jar cargo datagen downloaded are read"),
    );
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let mut unexplained = Vec::new();
    let (mut equal, mut other_height, mut other_column) = (0, 0, 0);
    for directory in &worlds {
        let world = World::read(directory);
        let generator = Scheduler::new(
            FeatureStages::with_templates(
                Arc::new(TerrainGenerator::new(world.seed)),
                Arc::clone(&templates),
            ),
            Options::for_processors(threads),
        );
        let ours = generator.spawn();
        let official = world.spawn;
        let origin = origin(world.seed);

        let verdict = if ours == official {
            equal += 1;
            "equal".to_owned()
        } else if (ours.x, ours.z) == (official.x, official.z) {
            // The same column at another height: a feature must stand on the official
            // column at or above Clustine's ground, a tree's canopy or snow.
            let (chunk, x, z) = column_of(official);
            let theirs = world.chunk(chunk);
            let mine = generator.generate(chunk);
            match feature_on(theirs, &mine, x, z, ours.y - 1) {
                Some(feature) => {
                    other_height += 1;
                    format!("other height, explained by {feature}")
                }
                None => {
                    unexplained.push(world.seed);
                    "OTHER HEIGHT, NO FEATURE ON THE COLUMN".to_owned()
                }
            }
        } else {
            // Another column: the first column tried that the two worlds judge
            // differently, wet in one and dry in the other, must show a feature in the
            // official world. Columns are tried up to whichever of the two spawns
            // comes later.
            let ours_at = column_of(ours);
            let theirs_at = column_of(official);
            let mut cache: BTreeMap<ChunkPos, Chunk> = BTreeMap::new();
            let mut explanation = None;
            for (chunk, x, z) in columns_in_order(origin) {
                let mine = cache
                    .entry(chunk)
                    .or_insert_with(|| generator.generate(chunk));
                let theirs = world.chunk(chunk);
                if feet(mine, x, z).is_some() != feet(theirs, x, z).is_some() {
                    explanation = Some(match feature_on(theirs, mine, x, z, i32::MIN) {
                        Some(feature) => Ok(format!(
                            "{feature} on the column {}, {}",
                            chunk.x * 16 + x as i32,
                            chunk.z * 16 + z as i32
                        )),
                        None => Err(format!(
                            "the column {}, {} differs without a feature",
                            chunk.x * 16 + x as i32,
                            chunk.z * 16 + z as i32
                        )),
                    });
                    break;
                }
                if (chunk, x, z) == ours_at || (chunk, x, z) == theirs_at {
                    break;
                }
            }
            match explanation {
                Some(Ok(explained)) => {
                    other_column += 1;
                    format!("other column, explained by {explained}")
                }
                Some(Err(why)) => {
                    unexplained.push(world.seed);
                    format!("OTHER COLUMN: {why}")
                }
                None => {
                    unexplained.push(world.seed);
                    "OTHER COLUMN, NO COLUMN TRIED BEFORE IS JUDGED DIFFERENTLY".to_owned()
                }
            }
        };
        println!(
            "judge two, seed {}: official {official:?}, Clustine {ours:?}: {verdict}",
            world.seed
        );
    }
    println!(
        "judge two: {equal} equal, {other_height} at another height and {other_column} on another column, explained; {} unexplained",
        unexplained.len()
    );
    assert!(
        unexplained.is_empty(),
        "the generator's spawn differs without a feature to explain it for seeds {unexplained:?}"
    );
}
