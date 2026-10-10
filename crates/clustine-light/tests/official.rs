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

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use clustine_data::BlockState;
use clustine_light::{Blocks, SectionLight, light};

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

enum Section {
    Uniform(BlockState),
    Mixed(Vec<BlockState>),
}

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

/// A chunk and the eight around it, as [`light`] wants them.
struct Nine<'a> {
    /// North-west first, row by row.
    chunks: [&'a Official; 9],
}

impl Nine<'_> {
    fn chunk(&self, chunk_x: i32, chunk_z: i32) -> &Official {
        self.chunks[((chunk_z + 1) * 3 + chunk_x + 1) as usize]
    }
}

impl Blocks for Nine<'_> {
    fn min_y(&self) -> i32 {
        self.chunks[4].lowest as i32 * 16
    }

    fn height(&self) -> u32 {
        self.chunks[4].sections.len() as u32 * 16
    }

    fn has_sky(&self) -> bool {
        true
    }

    fn has_chunk(&self, _chunk_x: i32, _chunk_z: i32) -> bool {
        true
    }

    fn state(&self, x: i32, y: i32, z: i32) -> BlockState {
        let from_bottom = (y - self.min_y()) as usize;
        match &self.chunk(x >> 4, z >> 4).sections[from_bottom / 16] {
            Section::Uniform(state) => *state,
            Section::Mixed(states) => {
                states[(from_bottom % 16) << 8 | ((z & 15) as usize) << 4 | (x & 15) as usize]
            }
        }
    }

    fn uniform_section(&self, chunk_x: i32, chunk_z: i32, section: usize) -> Option<BlockState> {
        match &self.chunk(chunk_x, chunk_z).sections[section] {
            Section::Uniform(state) => Some(*state),
            Section::Mixed(_) => None,
        }
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
}

#[test]
#[ignore = "needs the sample world of the official server"]
#[allow(
    clippy::disallowed_methods,
    reason = "the test, not the crate, takes the time"
)]
fn official_chunks_are_lit_as_the_official_server_lit_them() {
    let world = std::env::var("CLUSTINE_SAMPLE_WORLD")
        .expect("CLUSTINE_SAMPLE_WORLD names a world directory of the official server");
    let section_count = 24;
    let chunks = chunks_of(Path::new(&world), section_count);
    let full = chunks.values().filter(|chunk| chunk.full).count();
    println!("{} chunks read, {full} of them finished", chunks.len());

    let mut sky = Tally::default();
    let mut block = Tally::default();
    let mut compared = 0;
    let mut times = Vec::new();
    for (&(chunk_x, chunk_z), centre) in &chunks {
        let mut around = Vec::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                around.extend(chunks.get(&(chunk_x + dx, chunk_z + dz)).filter(|c| c.full));
            }
        }
        let Ok(nine) = <[&Official; 9]>::try_from(around) else {
            continue;
        };
        let nine = Nine { chunks: nine };
        let started = std::time::Instant::now();
        let made = light(&nine);
        times.push(started.elapsed());
        compared += 1;

        for (tally, kind, made, stored) in [
            (&mut sky, "sky", &made.sky, &centre.sky),
            (&mut block, "block", &made.block, &centre.block),
        ] {
            for (index, (made, stored)) in made.iter().zip(stored).enumerate() {
                let section_y = centre.lowest + index as i64 - 1;
                let place = format!("{kind} light, chunk {chunk_x} {chunk_z}, section {section_y}");
                match (stored, made.to_array()) {
                    (None, _) if *made == SectionLight::Absent => tally.absent += 1,
                    (None, _) if *made == SectionLight::Dark => tally.dark += 1,
                    (None, _) => {
                        tally.only_made += 1;
                        tally.show(format!("{place}: nothing stored, made {}", case(made)));
                    }
                    (Some(_), None) => {
                        tally.only_stored += 1;
                        tally.show(format!("{place}: an array stored, made absent"));
                    }
                    (Some(_), Some(_)) if *made == SectionLight::Dark => {
                        tally.only_stored += 1;
                        tally.show(format!("{place}: an array stored, made dark"));
                    }
                    (Some(stored), Some(array)) if stored[..] == array[..] => {
                        tally.equal_arrays += 1;
                        tally.full += usize::from(*made == SectionLight::Full);
                    }
                    (Some(stored), Some(array)) => {
                        tally.differing += 1;
                        let mut first = true;
                        for cell in 0..4096 {
                            let level = |of: &[u8]| of[cell / 2] >> (cell % 2 * 4) & 15;
                            if level(stored) == level(&array[..]) {
                                continue;
                            }
                            tally.differing_cells += 1;
                            if !first {
                                continue;
                            }
                            first = false;
                            let (x, y, z) = (cell & 15, cell >> 8, cell >> 4 & 15);
                            let world_y = section_y * 16 + y as i64;
                            let here = if (0..section_count as i64).contains(&(index as i64 - 1)) {
                                written(nine.state(x as i32, world_y as i32, z as i32))
                            } else {
                                "outside the world".to_owned()
                            };
                            tally.show(format!(
                                "{place}: at {x} {world_y} {z} ({here}) stored {} made {}",
                                level(stored),
                                level(&array[..]),
                            ));
                        }
                    }
                }
            }
        }
    }

    println!("{compared} chunks have eight finished neighbours and were lit and compared");
    for (kind, tally) in [("sky", &sky), ("block", &block)] {
        println!(
            "{kind} light: {} of {} sections match ({} equal arrays, {} of them full; \
             nothing stored and made dark {}, made absent {})",
            tally.matching(),
            tally.sections(),
            tally.equal_arrays,
            tally.full,
            tally.dark,
            tally.absent,
        );
        println!(
            "{kind} light: {} sections differ in {} cells; {} stored only; {} made only",
            tally.differing, tally.differing_cells, tally.only_stored, tally.only_made,
        );
        for line in &tally.shown {
            println!("  {line}");
        }
    }
    if !times.is_empty() {
        times.sort();
        let total: std::time::Duration = times.iter().sum();
        println!(
            "lighting one chunk: mean {:?}, median {:?}, least {:?}, most {:?}",
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
