//! The gate against the official server: over chunks it made and lit, this crate gives
//! the light it stored.
//!
//! Nothing of such a world is in the repository. The test takes the world's directory
//! from `CLUSTINE_SAMPLE_WORLD` and reads the overworld's region files itself: the
//! region format, NBT and 26.3's palette entries, as `docs/groundwork/terrain-g1.py`
//! reads them.
//!
//! ```text
//! CLUSTINE_SAMPLE_WORLD=<a world of the official server> \
//!     cargo test --release -p clustine-light --locked -- --ignored --nocapture official
//! ```
//!
//! The second comparison (`docs/groundwork/n8-plan.md`, section 9, the first of its
//! list) makes its own world: it starts the official server, changes blocks by commands
//! and reads what the server saved after each step, and has [`Lighting`] follow the same
//! changes on the same stored chunks. It needs Java and the jar of `cargo datagen`:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine-light --locked \
//!     --test official -- --ignored --nocapture light_after_changes
//! ```

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use clustine_data::BlockState;
use clustine_light::engine::{Chunks, Lighting};
use clustine_light::{Around, Scratch, Unlisted, VERSION, light, light_with};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, LIGHT_ARRAY_LENGTH, Light, SectionLight, light_tells_apart,
};

mod official_server;

// --- NBT, as the region files have it --------------------------------------------------

#[derive(Debug)]
enum Nbt {
    Number(i64),
    Real,
    Bytes(Vec<u8>),
    Text(String),
    List(Vec<Nbt>),
    Compound(BTreeMap<String, Nbt>),
    Ints,
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
                Nbt::Bytes(self.take(length).to_vec())
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
                self.take(4 * length);
                Nbt::Ints
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

#[derive(PartialEq)]
enum Section {
    Uniform(BlockState),
    Mixed(Vec<BlockState>),
}

#[derive(PartialEq)]
struct Official {
    full: bool,
    /// The section coordinate of the lowest section.
    lowest: i64,
    sections: Vec<Section>,
    /// By light section, from the one below the world: the stored array, if any.
    sky: Vec<Option<Vec<u8>>>,
    block: Vec<Option<Vec<u8>>>,
}

/// The state a palette entry names: a bare name for a block's default state, or a
/// compound of a name and properties; a list of both kinds wraps each in a compound.
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

fn official(chunk: &Nbt, section_count: usize) -> Official {
    let lowest = chunk.get("yPos").unwrap().number();
    let air = BlockState::parse("minecraft:air").unwrap();
    let mut read = Official {
        full: chunk
            .get("Status")
            .is_some_and(|status| status.text() == "minecraft:full"),
        lowest,
        sections: (0..section_count).map(|_| Section::Uniform(air)).collect(),
        sky: vec![None; section_count + 2],
        block: vec![None; section_count + 2],
    };
    for section in chunk.get("sections").map_or(&[][..], Nbt::list) {
        let y = section.get("Y").unwrap().number();
        let light_index = usize::try_from(y - lowest + 1).unwrap();
        for (key, into) in [("SkyLight", &mut read.sky), ("BlockLight", &mut read.block)] {
            if let Some(Nbt::Bytes(bytes)) = section.get(key) {
                assert_eq!(bytes.len(), 2048);
                into[light_index] = Some(bytes.clone());
            }
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
        let index = usize::try_from(y - lowest).unwrap();
        read.sections[index] = match states.get("data") {
            Some(Nbt::Longs(words)) => {
                let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(4) as usize;
                let in_a_word = 64 / bits;
                let mask = (1u64 << bits) - 1;
                Section::Mixed(
                    (0..4096)
                        .map(|at| {
                            let word = words[at / in_a_word] as u64;
                            palette[(word >> (at % in_a_word * bits) & mask) as usize]
                        })
                        .collect(),
                )
            }
            _ => Section::Uniform(palette[0]),
        };
    }
    read
}

/// Every chunk of the overworld's region files, by position.
fn chunks_of(world: &Path, section_count: usize) -> BTreeMap<(i64, i64), Official> {
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
                chunk.get("xPos").unwrap().number(),
                chunk.get("zPos").unwrap().number(),
            );
            found.insert(position, official(&chunk, section_count));
        }
    }
    found
}

impl Official {
    /// The chunk's blocks as a chunk of this server, which is what [`light`] is handed.
    /// The biome is of no account to light.
    fn chunk(&self) -> Chunk {
        let sections = self.sections.iter().map(|section| match section {
            Section::Uniform(state) => clustine_world::Section::filled(*state, Biome(0)),
            Section::Mixed(states) => {
                let states: Box<[BlockState; 4096]> =
                    states.clone().into_boxed_slice().try_into().unwrap();
                clustine_world::Section::from_states(states, Biome(0))
            }
        });
        Chunk::from_sections(self.lowest as i32 * 16, sections.collect())
    }
}

/// The 2,048 bytes a client is sent for a light section, if it is sent any.
fn array_of(light: &SectionLight) -> Option<Box<[u8; LIGHT_ARRAY_LENGTH]>> {
    match light {
        SectionLight::Absent => None,
        SectionLight::Dark => Some(Box::new([0; LIGHT_ARRAY_LENGTH])),
        SectionLight::Full => Some(Box::new([0xff; LIGHT_ARRAY_LENGTH])),
        SectionLight::Levels(levels) => Some(levels.clone()),
    }
}

fn written(state: BlockState) -> String {
    let name = state.block().map_or("?", |block| block.name);
    let properties: Vec<String> = state
        .properties()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if properties.is_empty() {
        name.to_owned()
    } else {
        format!("{name}[{}]", properties.join(","))
    }
}

/// What was found for one kind of light.
#[derive(Default)]
struct Tally {
    /// Sections whose stored array is what this crate made.
    equal_arrays: usize,
    /// Of those, the ones that are 15 throughout, which this crate calls full.
    full: usize,
    /// Sections without a stored array that this crate has dark or absent.
    dark: usize,
    absent: usize,
    /// Sections whose stored array differs from this crate's levels.
    differing: usize,
    differing_cells: usize,
    /// Sections with a stored array where this crate has none or a dark one.
    only_stored: usize,
    /// Sections without a stored array where this crate has levels.
    only_made: usize,
    /// The first differences, written out.
    shown: Vec<String>,
}

impl Tally {
    fn sections(&self) -> usize {
        self.matching() + self.differing + self.only_stored + self.only_made
    }

    fn matching(&self) -> usize {
        self.equal_arrays + self.dark + self.absent
    }

    fn show(&mut self, line: String) {
        if self.shown.len() < 24 {
            self.shown.push(line);
        }
    }

    fn mismatches(&self) -> usize {
        self.differing + self.only_stored + self.only_made
    }

    fn count(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Absent => self.absent += 1,
            Outcome::Dark => self.dark += 1,
            Outcome::Equal { full } => {
                self.equal_arrays += 1;
                self.full += usize::from(full);
            }
            Outcome::OnlyMade(line) => {
                self.only_made += 1;
                self.show(line);
            }
            Outcome::OnlyStored(line) => {
                self.only_stored += 1;
                self.show(line);
            }
            Outcome::Differs { cells, first } => {
                self.differing += 1;
                self.differing_cells += cells;
                self.show(first);
            }
        }
    }

    fn print(&self, kind: &str) {
        println!(
            "{kind} light: {} of {} sections match ({} equal arrays, {} of them full; \
             nothing stored and made dark {}, made absent {})",
            self.matching(),
            self.sections(),
            self.equal_arrays,
            self.full,
            self.dark,
            self.absent,
        );
        println!(
            "{kind} light: {} sections differ in {} cells; {} stored only; {} made only",
            self.differing, self.differing_cells, self.only_stored, self.only_made,
        );
        for line in &self.shown {
            println!("  {line}");
        }
    }
}

/// How one light section of this crate compares with the one the official server
/// stored, a mismatch written out with its place.
enum Outcome {
    Absent,
    Dark,
    Equal {
        full: bool,
    },
    OnlyMade(String),
    OnlyStored(String),
    /// The arrays differ in `cells` cells; `first` says where the first is, its block and
    /// the level on each side.
    Differs {
        cells: usize,
        first: String,
    },
}

/// Compares the section of light index `index` of `own` (the chunk the light is of, as
/// this crate has its blocks): what this crate made, against what was stored. `place`
/// names the kind of light and the chunk.
fn compare_section(
    place: &str,
    lowest: i64,
    index: usize,
    made: &SectionLight,
    stored: Option<&Vec<u8>>,
    own: &Chunk,
) -> Outcome {
    let section_y = lowest + index as i64 - 1;
    let place = format!("{place}, section {section_y}");
    match (stored, array_of(made)) {
        (None, _) if *made == SectionLight::Absent => Outcome::Absent,
        (None, _) if *made == SectionLight::Dark => Outcome::Dark,
        (None, _) => Outcome::OnlyMade(format!("{place}: nothing stored, made {}", case(made))),
        (Some(_), None) => Outcome::OnlyStored(format!("{place}: an array stored, made absent")),
        // A section made dark against an array stored is compared by its levels: the
        // game keeps an array of zeros where light has gone from a section, and a client
        // shows the same as for one sent dark; an array holding any light differs, cell
        // by cell, below.
        (Some(stored), Some(array)) if stored[..] == array[..] => Outcome::Equal {
            full: *made == SectionLight::Full,
        },
        (Some(stored), Some(array)) => {
            let level = |of: &[u8], cell: usize| of[cell / 2] >> (cell % 2 * 4) & 15;
            let differing: Vec<usize> = (0..4096)
                .filter(|&cell| level(stored, cell) != level(&array[..], cell))
                .collect();
            let cell = differing[0];
            let (x, y, z) = (cell & 15, cell >> 8, cell >> 4 & 15);
            let world_y = section_y * 16 + y as i64;
            let here = own
                .get(x, world_y as i32, z)
                .map_or("outside the world".to_owned(), written);
            Outcome::Differs {
                cells: differing.len(),
                first: format!(
                    "{place}: at {x} {world_y} {z} in the chunk ({here}) stored {} made {}, \
                     {} cells differ",
                    level(stored, cell),
                    level(&array[..], cell),
                    differing.len(),
                ),
            }
        }
    }
}

#[test]
#[ignore = "needs the sample world of the official server"]
#[allow(
    clippy::disallowed_methods,
    reason = "the test, not the crate, takes the time"
)]
fn official_chunks_are_lit_as_the_official_server_lit_them() {
    // Without a sample world there is nothing to compare with. The test is run by
    // name with the other comparisons, most of which need no such world, and is not
    // to fail them.
    let Ok(world) = std::env::var("CLUSTINE_SAMPLE_WORLD") else {
        println!("skipped: CLUSTINE_SAMPLE_WORLD names no world directory of the official server");
        return;
    };
    let section_count = 24;
    let chunks = chunks_of(Path::new(&world), section_count);
    let full = chunks.values().filter(|chunk| chunk.full).count();
    println!("{} chunks read, {full} of them finished", chunks.len());

    // The finished chunks as chunks of this server, made before any time is taken.
    let finished: BTreeMap<(i64, i64), Chunk> = chunks
        .iter()
        .filter(|(_, chunk)| chunk.full)
        .map(|(position, chunk)| (*position, chunk.chunk()))
        .collect();

    let mut sky = Tally::default();
    let mut block = Tally::default();
    let mut compared = 0;
    let mut times = Vec::new();
    // The same chunks lit as a thread of an edge lights them: in one scratch, kept
    // from chunk to chunk.
    let mut scratch = Scratch::new();
    let mut times_in_scratch = Vec::new();
    for (&(chunk_x, chunk_z), centre) in &chunks {
        // North-west first, row by row: north is the lower z.
        let mut nine = Vec::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                nine.extend(finished.get(&(chunk_x + dx, chunk_z + dz)));
            }
        }
        let Ok([nw, n, ne, w, own, e, sw, s, se]) = <[&Chunk; 9]>::try_from(nine) else {
            continue;
        };
        let around = Around {
            centre: own,
            neighbours: [nw, n, ne, w, e, sw, s, se].map(Some),
            sky: true,
        };
        let started = std::time::Instant::now();
        let made = light(&around);
        times.push(started.elapsed());
        let started = std::time::Instant::now();
        let (again, unlisted) = light_with(&around, Unlisted::NONE, &mut scratch);
        times_in_scratch.push(started.elapsed());
        assert!(
            again == made,
            "chunk {chunk_x} {chunk_z} is lit otherwise in a used scratch"
        );
        assert!(unlisted.is_empty());
        compared += 1;
        assert_eq!(made.made_by, VERSION);
        assert_eq!(made.sky.len(), section_count + 2);
        assert_eq!(made.block.len(), section_count + 2);

        for (tally, kind, made, stored) in [
            (&mut sky, "sky", &made.sky, &centre.sky),
            (&mut block, "block", &made.block, &centre.block),
        ] {
            for (index, (made, stored)) in made.iter().zip(stored).enumerate() {
                let place = format!("{kind} light, chunk {chunk_x} {chunk_z}");
                tally.count(compare_section(
                    &place,
                    centre.lowest,
                    index,
                    made,
                    stored.as_ref(),
                    own,
                ));
            }
        }
    }

    println!("{compared} chunks have eight finished neighbours and were lit and compared");
    sky.print("sky");
    block.print("block");
    for (how, mut times) in [
        ("lighting one chunk", times),
        (
            "lighting one chunk in a scratch used before",
            times_in_scratch,
        ),
    ] {
        if times.is_empty() {
            continue;
        }
        times.sort();
        let total: std::time::Duration = times.iter().sum();
        println!(
            "{how}: mean {:?}, median {:?}, least {:?}, most {:?}",
            total / times.len() as u32,
            times[times.len() / 2],
            times[0],
            times[times.len() - 1],
        );
    }
    assert!(
        compared > 0,
        "no chunk of the world has eight finished neighbours"
    );
    for tally in [&sky, &block] {
        assert_eq!(
            tally.matching(),
            tally.sections(),
            "light differs; see above"
        );
    }
}

fn case(light: &SectionLight) -> &'static str {
    match light {
        SectionLight::Absent => "absent",
        SectionLight::Dark => "dark",
        SectionLight::Full => "full",
        SectionLight::Levels(_) => "levels",
    }
}

// --- Light after changes, against the official server's ---------------------------------

/// The script of block changes, step by step, each by commands on the official server's
/// console. The ground's top is at y -1 (`official_server::PROPERTIES`); the scene lies
/// within x and z -40 to 39, the chunks -3 to 2, so that every chunk it lights is
/// compared with eight finished neighbours. Each step is saved and compared on its own,
/// so that a difference names the step that made it.
const STEPS: &[(&str, &[&str])] = &[
    ("the ground as generated", &[]),
    (
        "torches, one beside a chunk's border",
        &[
            "setblock 5 0 5 minecraft:torch",
            "setblock 15 0 3 minecraft:torch",
            "setblock 8 0 -12 minecraft:torch",
        ],
    ),
    (
        "glowstone in the air and in the ground",
        &[
            "setblock 3 4 -6 minecraft:glowstone",
            "setblock -6 -1 -6 minecraft:glowstone",
        ],
    ),
    (
        "glass: a box round a torch, and a pane in the air",
        &[
            "fill 10 0 10 14 4 14 minecraft:glass hollow",
            "setblock 12 0 12 minecraft:torch",
            "fill 20 3 20 26 3 26 minecraft:glass",
        ],
    ),
    (
        "a roof on a wall, and a lantern under it",
        &[
            "fill -12 6 -12 -3 6 -3 minecraft:stone",
            "fill -12 0 -12 -12 5 -3 minecraft:stone",
            "setblock -8 0 -8 minecraft:lantern",
        ],
    ),
    (
        "a cave under the ground, across a chunk's border, with a torch in it",
        &[
            "fill 18 -30 -12 30 -24 0 minecraft:air",
            "setblock 20 -30 -10 minecraft:torch",
        ],
    ),
    (
        "a hole dug from the ground to the cave",
        &["fill 24 -23 -6 24 -1 -6 minecraft:air"],
    ),
    (
        "water: a pool in the ground, and a source poured beside it that flows",
        &[
            "fill -25 -3 15 -18 -1 22 minecraft:water",
            "setblock -10 0 25 minecraft:water",
        ],
    ),
    (
        "a floating platform of stone, and one of leaves lower down",
        &[
            "fill -40 100 -40 -30 100 -30 minecraft:stone",
            "fill 35 30 -40 39 30 -36 minecraft:oak_leaves[persistent=true]",
        ],
    ),
    (
        "blocks across a chunk's corner: glowstone, a slab, stairs and a roof; and \
         glowstone across the corner of four chunks and two sections",
        &[
            "fill -1 0 -1 0 2 0 minecraft:glowstone",
            "setblock -1 3 0 minecraft:stone_slab[type=bottom]",
            "setblock 0 3 -1 minecraft:oak_stairs[facing=north,half=bottom]",
            "fill -3 5 -3 2 5 2 minecraft:stone",
            "setblock 16 15 16 minecraft:glowstone",
            "setblock 15 16 15 minecraft:glowstone",
            "setblock 15 16 16 minecraft:stone",
            "setblock 16 16 15 minecraft:stone",
        ],
    ),
    (
        "taken away: a torch, the roof, a hole in the platform, one glowstone of the \
         corner, and the cave's hole capped",
        &[
            "setblock 5 0 5 minecraft:air",
            "fill -12 6 -12 -3 6 -3 minecraft:air",
            "fill -36 100 -36 -34 100 -34 minecraft:air",
            "setblock -1 0 -1 minecraft:air",
            "setblock 24 -1 -6 minecraft:stone",
        ],
    ),
];

/// The chunks kept loaded, from -5 to 4 each way: the scene's, and two rings round them
/// so that each of the scene's has eight finished neighbours.
const LOADED: std::ops::RangeInclusive<i64> = -5..=4;
const LOADED_BLOCKS: &str = "forceload add -80 -80 79 79";

/// The chunks the scene lights, which must be compared at every step.
const SCENE: std::ops::RangeInclusive<i64> = -3..=2;

/// The sections of the overworld.
const SECTIONS: usize = 24;

/// Game ticks between two saves that must agree before the world counts as settled.
const SETTLE_TICKS: u64 = 20;

/// The most pairs of saves the official server is given to settle after a step.
const SETTLE_ROUNDS: usize = 40;

/// Runs one command of the script, telling by its verb what answers it.
fn run(server: &mut official_server::Server, command: &str) {
    let verb = command.split(' ').next().unwrap_or_default();
    let (succeeded, failed): (&[&str], &[&str]) = match verb {
        "setblock" => (&["Changed the block"], &["Could not set the block"]),
        "fill" => (
            &["Successfully filled"],
            &["No blocks were filled", "Too many blocks"],
        ),
        "forceload" => (&["to be force loaded"], &["No chunks were marked"]),
        "gamerule" => (&["is now set to"], &[]),
        "kill" => (&["Killed", "No entity was found"], &[]),
        other => panic!("the script has no answer for `{other}`"),
    };
    server.command(command, succeeded, failed);
}

/// The finished chunks of a save.
fn finished(world: &Path) -> BTreeMap<(i64, i64), Official> {
    chunks_of(world, SECTIONS)
        .into_iter()
        .filter(|(_, chunk)| chunk.full)
        .collect()
}

/// Saves until two saves `SETTLE_TICKS` ticks apart agree in every finished chunk's blocks
/// and light and every loaded chunk is finished: the official server lights on a thread
/// of its own and water flows for some ticks, so the state is waited for, not time.
/// Returns the save and the game time it was made at.
fn settle(
    server: &mut official_server::Server,
    step: &str,
) -> (BTreeMap<(i64, i64), Official>, u64) {
    let mut last: Option<BTreeMap<(i64, i64), Official>> = None;
    let mut unsettled = String::new();
    for _ in 0..SETTLE_ROUNDS {
        server.let_ticks_pass(SETTLE_TICKS);
        server.save();
        let time = server.game_time();
        let saved = finished(server.world());
        let missing: Vec<(i64, i64)> = LOADED
            .flat_map(|x| LOADED.map(move |z| (x, z)))
            .filter(|at| !saved.contains_key(at))
            .collect();
        let changed: Vec<(i64, i64)> = match &last {
            None => saved.keys().copied().collect(),
            Some(last) => saved
                .iter()
                .filter(|(at, chunk)| last.get(at) != Some(chunk))
                .map(|(at, _)| *at)
                .chain(last.keys().filter(|at| !saved.contains_key(at)).copied())
                .collect(),
        };
        if missing.is_empty() && changed.is_empty() {
            return (saved, time);
        }
        unsettled = format!(
            "at game time {time}, {} loaded chunks not finished (first {:?}), {} chunks \
             changed since the save before (first {:?})",
            missing.len(),
            missing.first(),
            changed.len(),
            changed.first(),
        );
        last = Some(saved);
    }
    panic!(
        "after `{step}` the official server's world did not settle in {SETTLE_ROUNDS} \
         saves {SETTLE_TICKS} ticks apart: {unsettled}"
    );
}

/// The chunks this crate's engine reads: those the official server finished.
struct Held(BTreeMap<ChunkPos, Chunk>);

impl Chunks for Held {
    fn chunk(&self, at: ChunkPos) -> Option<&Chunk> {
        self.0.get(&at)
    }
}

fn chunk_pos((x, z): (i64, i64)) -> ChunkPos {
    ChunkPos::new(x as i32, z as i32)
}

/// Makes the held chunks those of `saved`, block by block where a chunk was held before
/// (`before` is the save they were made from), telling `lighting` each change light can
/// see, as a region does. Returns how many blocks changed.
fn follow(
    held: &mut Held,
    lighting: &mut Lighting,
    before: &BTreeMap<(i64, i64), Official>,
    saved: &BTreeMap<(i64, i64), Official>,
) -> usize {
    let mut changes = 0;
    for gone in before.keys().filter(|at| !saved.contains_key(at)) {
        held.0.remove(&chunk_pos(*gone));
        lighting.left(chunk_pos(*gone));
    }
    for (&(chunk_x, chunk_z), official) in saved {
        let at = chunk_pos((chunk_x, chunk_z));
        let Some(old) = before.get(&(chunk_x, chunk_z)) else {
            held.0.insert(at, official.chunk());
            lighting.arrived(at);
            continue;
        };
        let chunk = held.0.get_mut(&at).expect("a chunk saved before is held");
        for (index, (was, is)) in old.sections.iter().zip(&official.sections).enumerate() {
            if was == is {
                continue;
            }
            for cell in 0..4096 {
                let state = match is {
                    Section::Uniform(state) => *state,
                    Section::Mixed(states) => states[cell],
                };
                let (x, y, z) = (cell & 15, cell >> 8, cell >> 4 & 15);
                let world_y = (official.lowest + index as i64) as i32 * 16 + y as i32;
                let old = chunk
                    .set(x, world_y, z, state)
                    .expect("the cell is in the chunk");
                if old == state {
                    continue;
                }
                changes += 1;
                if light_tells_apart(old, state) {
                    lighting.changed(BlockPos::new(
                        chunk_x as i32 * 16 + x as i32,
                        world_y,
                        chunk_z as i32 * 16 + z as i32,
                    ));
                }
            }
        }
    }
    changes
}

/// The first light section in which two lights differ, written out.
fn first_difference(engine: &Light, function: &Light) -> String {
    for (kind, engine, function) in [
        ("sky", &engine.sky, &function.sky),
        ("block", &engine.block, &function.block),
    ] {
        if engine.len() != function.len() {
            return format!(
                "{kind}: {} sections against {}",
                engine.len(),
                function.len()
            );
        }
        for (index, (engine, function)) in engine.iter().zip(function).enumerate() {
            if engine != function {
                return format!(
                    "{kind}, light section {index}: the engine has {}, the function {}",
                    case(engine),
                    case(function)
                );
            }
        }
    }
    "the version or `beside`".to_owned()
}

/// What one step's comparison found.
#[derive(Default)]
struct Found {
    sky: Tally,
    block: Tally,
    /// Sky light sections that differ under something floating: the crate's named
    /// difference (`lib.rs`, "Where this is not the game"), listed apart.
    floating: Tally,
    compared: Vec<(i64, i64)>,
    /// Where the engine's light is not the crate's function's of the same blocks.
    engine: Vec<String>,
}

/// Compares the engine's light of every held chunk with eight held neighbours with what
/// the official server saved, and with the crate's function of the same blocks.
fn compare(
    label: &str,
    held: &Held,
    lighting: &Lighting,
    saved: &BTreeMap<(i64, i64), Official>,
) -> Found {
    let mut found = Found::default();
    for (&(chunk_x, chunk_z), official) in saved {
        let at = chunk_pos((chunk_x, chunk_z));
        let mut nine = Vec::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                nine.extend(held.0.get(&ChunkPos::new(at.x + dx, at.z + dz)));
            }
        }
        let Ok([nw, n, ne, w, own, e, sw, s, se]) = <[&Chunk; 9]>::try_from(nine.clone()) else {
            continue;
        };
        found.compared.push((chunk_x, chunk_z));
        let made = lighting
            .light(at, held)
            .expect("a held chunk is present in the engine");
        let function = light(&Around {
            centre: own,
            neighbours: [nw, n, ne, w, e, sw, s, se].map(Some),
            sky: true,
        });
        if made != function {
            found.engine.push(format!(
                "{label}: chunk {chunk_x} {chunk_z}: the engine is not the function: {}",
                first_difference(&made, &function)
            ));
        }

        // Which sections of the nine hold a block, for the named case: a light section
        // with an unlisted one above it, and a block above that, is under something
        // floating, where the sky's light comes a way the game does not take.
        let mut occupied = [false; SECTIONS];
        for chunk in &nine {
            for (index, section) in chunk.sections().iter().enumerate() {
                occupied[index] |= section.non_air_count() > 0;
            }
        }
        let under_floating = |index: usize| {
            (index + 1..made.sky.len()).any(|gap| {
                made.sky[gap] == SectionLight::Absent
                    && occupied
                        .iter()
                        .enumerate()
                        .any(|(section, held)| *held && section + 1 > gap)
            })
        };

        for (kind, made, stored) in [
            ("sky", &made.sky, &official.sky),
            ("block", &made.block, &official.block),
        ] {
            for (index, (made, stored)) in made.iter().zip(stored).enumerate() {
                let place = format!("{label}: {kind} light, chunk {chunk_x} {chunk_z}");
                let outcome =
                    compare_section(&place, official.lowest, index, made, stored.as_ref(), own);
                let mismatch = !matches!(
                    outcome,
                    Outcome::Absent | Outcome::Dark | Outcome::Equal { .. }
                );
                if mismatch && kind == "sky" && under_floating(index) {
                    found.floating.count(outcome);
                } else if kind == "sky" {
                    found.sky.count(outcome);
                } else {
                    found.block.count(outcome);
                }
            }
        }
    }
    found
}

/// Follows one save with the engine, compares, prints what was found and gathers what
/// fails. Returns the number of sections that differed under something floating.
fn check(
    label: &str,
    saved: &BTreeMap<(i64, i64), Official>,
    held: &mut Held,
    lighting: &mut Lighting,
    before: &BTreeMap<(i64, i64), Official>,
    failures: &mut Vec<String>,
) -> usize {
    let changes = follow(held, lighting, before, saved);
    let work = lighting.run(held);
    let found = compare(label, held, lighting, saved);
    println!(
        "{label}: {changes} blocks changed; the engine lit {} chunks whole and worked off \
         {} cells; {} chunks compared",
        work.chunks_lit_whole,
        work.cells,
        found.compared.len()
    );
    found.sky.print("  sky");
    found.block.print("  block");
    if found.floating.sections() > 0 {
        println!("  under something floating, the crate's named difference, listed apart:");
        found.floating.print("    sky");
    }
    let missing: Vec<(i64, i64)> = SCENE
        .flat_map(|x| SCENE.map(move |z| (x, z)))
        .filter(|at| !found.compared.contains(at))
        .collect();
    if !missing.is_empty() {
        failures.push(format!(
            "{label}: the scene's chunks {missing:?} were not compared"
        ));
    }
    failures.extend(found.engine.iter().cloned());
    for tally in [&found.sky, &found.block] {
        if tally.mismatches() > 0 {
            failures.extend(tally.shown.iter().cloned());
        }
    }
    found.floating.sections()
}

#[test]
#[ignore = "needs Java and the official server's jar"]
fn light_after_changes_is_what_the_official_server_stored() {
    let mut server = official_server::Server::start();
    // Nothing but the script changes a block: no random tick turns grass under a roof
    // to dirt, and no mob is about.
    for command in [
        "gamerule random_tick_speed 0",
        "gamerule spawn_mobs false",
        "kill @e[type=!minecraft:player]",
        LOADED_BLOCKS,
    ] {
        run(&mut server, command);
    }

    let mut held = Held(BTreeMap::new());
    let mut lighting = Lighting::new(true);
    let mut before = BTreeMap::new();
    let mut failures = Vec::new();
    let mut floating = 0;
    for (step, commands) in STEPS {
        for command in *commands {
            run(&mut server, command);
        }
        let (saved, time) = settle(&mut server, step);
        let label = format!("`{step}` (saved at game time {time})");
        floating += check(
            &label,
            &saved,
            &mut held,
            &mut lighting,
            &before,
            &mut failures,
        );
        before = saved;
    }
    // What the server keeps when it stops is what is compared last.
    let world = server.world().to_owned();
    server.stop();
    let saved = finished(&world);
    floating += check(
        "the world as the server stopped it",
        &saved,
        &mut held,
        &mut lighting,
        &before,
        &mut failures,
    );

    println!("{floating} sky light sections differed under something floating, listed apart");
    assert!(
        failures.is_empty(),
        "light after changes differs from the official server's ({} lines; each names the \
         step, the chunk, the section and its first differing cell):\n{}",
        failures.len(),
        failures
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
