//! The judge of features (`docs/groundwork/f-plan.md`, step F0; the rules of ADR-0021's
//! section on testing): the official server places the features of an area in
//! Clustine's order of runs, inside a Java program (`tests/java/GameFeatures.java`), and
//! every chunk of the area whose runs all lay inside it is compared with Clustine's:
//! blocks, block entities, ticks, marks and the four final heightmaps.
//!
//! The features whose kinds Clustine has not built are turned into the game's `no_op`
//! by a data pack written here from Clustine's own reading of the data, keeping every
//! feature's place in the order and so its seed: each step of the plan is judged
//! exactly on what is built so far. Structures are off on both sides until phase S.
//! The program runs twice, placing each rank's chunks once in ascending and once in
//! descending order; runs of one rank do not meet, so the two must agree.
//!
//! It needs a JDK 25 (`javac` and `java` on the path), the jar `cargo datagen`
//! unpacked, and the owner's acceptance of the Minecraft EULA:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test --release -p clustine-worldgen \
//!     --test game_features -- --ignored --nocapture
//! ```
//!
//! `CLUSTINE_GAME_FEATURES_SEED` (13579 unless said) and `CLUSTINE_GAME_FEATURES_AREA`
//! (`x0 z0 x1 z1`, the chunks whose features run, both corners included) choose what
//! is judged. The area has to lie well away from the spawn, whose search makes the
//! chunks it looks at whole before the program has the server; the program fails if
//! it finds one decorated before it asked.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use clustine_data::{BLOCK_ENTITY_TYPES, BLOCKS, BlockState, FLUIDS};
use clustine_features::feature::{Feature, read_configured};
use clustine_features::reader::Reader;
use clustine_protocol::nbt::Nbt;
use clustine_world::{Chunk, ChunkPos, Ticked};
use clustine_worldgen::scheduler::{Options, Scheduler};
use clustine_worldgen::{FeatureStages, TerrainGenerator};
use clustine_worldgen_data::feature::{Data, configured_features};

/// Clustine's ranks of runs (ADR-0021), by x mod 3 and then z mod 3.
const RANKS: [[u8; 3]; 3] = [[0, 1, 2], [7, 6, 3], [8, 5, 4]];

fn rank(chunk: ChunkPos) -> u8 {
    RANKS[chunk.x.rem_euclid(3) as usize][chunk.z.rem_euclid(3) as usize]
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The area whose runs the program makes.
#[derive(Debug, Clone, Copy)]
struct Area {
    min: ChunkPos,
    max: ChunkPos,
}

impl Area {
    fn contains(&self, chunk: ChunkPos) -> bool {
        (self.min.x..=self.max.x).contains(&chunk.x) && (self.min.z..=self.max.z).contains(&chunk.z)
    }

    fn chunks(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        (self.min.x..=self.max.x)
            .flat_map(move |x| (self.min.z..=self.max.z).map(move |z| ChunkPos::new(x, z)))
    }

    /// Whether every run the finished chunk `e` rests on lies in the area: the runs
    /// within one of it, and every run each of those sees, which is a run of a lower
    /// rank within two of it (ADR-0021, definition D).
    fn holds_the_runs_of(&self, e: ChunkPos) -> bool {
        let mut waiting: Vec<ChunkPos> = around(e, 1).collect();
        let mut seen = waiting.clone();
        while let Some(run) = waiting.pop() {
            if !self.contains(run) {
                return false;
            }
            for seen_by in around(run, 2).filter(|d| rank(*d) < rank(run)) {
                if !seen.contains(&seen_by) {
                    seen.push(seen_by);
                    waiting.push(seen_by);
                }
            }
        }
        true
    }
}

fn around(centre: ChunkPos, radius: i32) -> impl Iterator<Item = ChunkPos> {
    (-radius..=radius).flat_map(move |dx| {
        (-radius..=radius).map(move |dz| ChunkPos::new(centre.x + dx, centre.z + dz))
    })
}

/// A value of the data written as JSON, with the kinds of features that are not
/// built turned into `no_op`. `feature` says whether the value stands where a
/// configured feature does: at the top of a file, or as the `feature` of a placed
/// feature written out.
fn json(out: &mut String, data: &'static Data, feature: bool) -> bool {
    let mut changed = false;
    match data {
        Data::Bool(value) => write!(out, "{value}").unwrap(),
        Data::Number(text) => out.push_str(text),
        Data::Text(text) => write!(out, "{text:?}").unwrap(),
        Data::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                changed |= json(out, item, false);
            }
            out.push(']');
        }
        Data::Object(members) => {
            if feature && !read(data).built() {
                out.push_str(r#"{"type":"minecraft:no_op"}"#);
                return true;
            }
            let placed = data.get("feature").is_some() && data.get("placement").is_some();
            out.push('{');
            for (index, (key, value)) in members.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write!(out, "{key:?}:").unwrap();
                changed |= json(out, value, placed && *key == "feature");
            }
            out.push('}');
        }
    }
    changed
}

fn read(data: &'static Data) -> Box<dyn Feature> {
    read_configured(&Reader::new(data, "the judge's pack"))
        .expect("every feature of the data is read whole")
}

/// The data pack that turns the features whose kinds are not built into `no_op`:
/// every configured feature that is or holds one, written again.
fn pack(directory: &Path) -> usize {
    let mut written = 0;
    for entry in configured_features() {
        let mut text = String::new();
        if !json(&mut text, &entry.data, true) {
            continue;
        }
        let path = entry
            .name
            .strip_prefix("minecraft:")
            .expect("the game's own");
        let file = directory.join(format!("data/minecraft/worldgen/feature/{path}.json"));
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
        written += 1;
    }
    fs::write(
        directory.join("pack.mcmeta"),
        r#"{"pack":{"description":"Clustine: the features it has not built","min_format":121,"max_format":121}}"#,
    )
    .unwrap();
    written
}

fn classpath() -> String {
    let mut jars = Vec::new();
    let mut directories = vec![root().join("target/datagen/26.3/extract/classpath")];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).expect("cargo datagen unpacked the jar") {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "jar") {
                jars.push(path.display().to_string());
            }
        }
    }
    jars.sort();
    jars.join(":")
}

/// Runs the program in a server directory of its own, and returns what it wrote.
fn game(seed: i64, area: Area, ascending: bool, classes: &Path, pack: &Path) -> String {
    let server = tempfile::Builder::new()
        .prefix("game-features")
        .tempdir()
        .unwrap();
    let directory = server.path();
    fs::write(directory.join("eula.txt"), "eula=true\n").unwrap();
    fs::write(
        directory.join("server.properties"),
        format!(
            "level-seed={seed}\ngenerate-structures=false\nonline-mode=false\n\
             server-port=0\nquery.port=0\ndifficulty=peaceful\nspawn-monsters=false\n\
             pause-when-empty-seconds=-1\nmax-tick-time=-1\nsync-chunk-writes=false\n\
             initial-enabled-packs=vanilla,file/clustine-unbuilt\n"
        ),
    )
    .unwrap();
    let datapacks = directory.join("world/datapacks/clustine-unbuilt");
    copy(pack, &datapacks);
    fs::write(
        directory.join("area"),
        format!(
            "{} {} {} {}",
            area.min.x, area.min.z, area.max.x, area.max.z
        ),
    )
    .unwrap();
    let output = directory.join("output");
    let log = fs::File::create(directory.join("log")).unwrap();
    let mut child = Command::new("java")
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), classpath()))
        .arg("GameFeatures")
        .arg("area")
        .arg(if ascending { "ascending" } else { "descending" })
        .arg(&output)
        .current_dir(directory)
        // The server is found through the thread that reads its console, which ends
        // when the input closes: it is kept open until the program has ended.
        .stdin(Stdio::piped())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .expect("java is on the path");
    let input = child.stdin.take();
    let status = child.wait().unwrap();
    drop(input);
    let said = fs::read_to_string(directory.join("log")).unwrap_or_default();
    assert!(
        status.success(),
        "the program failed ({status}); the end of what it said:\n{}",
        said.lines()
            .rev()
            .take(30)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    );
    fs::read_to_string(output).expect("the program wrote its output")
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// A block's position.
type At = (i32, i32, i32);

/// A scheduled tick as the game saves it: where, of what, its delay and priority.
type Tick = (At, String, i32, i32);

/// What the game wrote of one chunk, in the form Clustine's chunk is put in to be
/// compared.
#[derive(Debug, Default, PartialEq)]
struct Written {
    blocks: Vec<u16>,
    /// By position: the type and the data, compounds sorted by key.
    entities: BTreeMap<At, (String, Option<Nbt>)>,
    /// Sorted: position, what, delay, priority.
    ticks: Vec<Tick>,
    marks: Vec<At>,
    heights: Vec<(String, Vec<i32>)>,
}

fn position(text: &str) -> (i32, i32, i32) {
    let mut parts = text.split(',').map(|part| part.parse::<i32>().unwrap());
    (
        parts.next().unwrap(),
        parts.next().unwrap(),
        parts.next().unwrap(),
    )
}

/// An NBT value with every compound's entries in the order of their keys, since the
/// game writes them in the order of a hash map.
fn sorted(value: Nbt) -> Nbt {
    match value {
        Nbt::Compound(mut entries) => {
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Nbt::Compound(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            )
        }
        Nbt::List(items) => Nbt::List(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn nbt(bytes: &[u8]) -> Option<Nbt> {
    if bytes.is_empty() {
        return None;
    }
    clustine_protocol::codec::Reader::new(bytes)
        .nbt()
        .expect("block entity data is NBT")
        .map(sorted)
        .filter(|value| !matches!(value, Nbt::Compound(entries) if entries.is_empty()))
}

fn parse(text: &str) -> BTreeMap<ChunkPos, Written> {
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("game-features 1"));
    let mut chunks = BTreeMap::new();
    let mut current: Option<(ChunkPos, Written)> = None;
    for line in lines {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let written = match word {
            "chunk" => {
                if let Some((at, written)) = current.take() {
                    chunks.insert(at, written);
                }
                let mut parts = rest.split(' ').map(|part| part.parse().unwrap());
                let at = ChunkPos::new(parts.next().unwrap(), parts.next().unwrap());
                current = Some((at, Written::default()));
                continue;
            }
            _ => &mut current.as_mut().expect("a chunk comes first").1,
        };
        match word {
            "blocks" => {
                for run in rest.split(' ') {
                    let (id, count) = run.split_once('*').unwrap();
                    let id: u16 = id.parse().unwrap();
                    written
                        .blocks
                        .extend(std::iter::repeat_n(id, count.parse().unwrap()));
                }
            }
            "entity" => {
                let mut parts = rest.split(' ');
                let at = position(parts.next().unwrap());
                let kind = parts.next().unwrap().to_owned();
                let hex = parts.next().unwrap_or("");
                let bytes: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
                    .collect();
                written.entities.insert(at, (kind, nbt(&bytes)));
            }
            "tick" => {
                let parts: Vec<&str> = rest.split(' ').collect();
                written.ticks.push((
                    position(parts[1]),
                    parts[2].to_owned(),
                    parts[3].parse().unwrap(),
                    parts[4].parse().unwrap(),
                ));
            }
            "marks" => {
                written.marks = rest
                    .split(' ')
                    .filter(|part| !part.is_empty())
                    .map(position)
                    .collect();
            }
            "height" => {
                let mut parts = rest.split(' ');
                let name = parts.next().unwrap().to_owned();
                written
                    .heights
                    .push((name, parts.map(|part| part.parse().unwrap()).collect()));
            }
            other => panic!("the program wrote a line of {other:?}"),
        }
    }
    if let Some((at, written)) = current {
        chunks.insert(at, written);
    }
    for written in chunks.values_mut() {
        written.ticks.sort();
        // The game appends a mark to its list without looking for it there, and a
        // position processed twice is what it is processed once; Clustine's chunk keeps
        // its marks as a set (ADR-0022).
        written.marks.sort_unstable();
        written.marks.dedup();
    }
    chunks
}

/// Clustine's chunk in the form of what the game wrote.
fn ours(chunk: &Chunk, at: ChunkPos) -> Written {
    let mut written = Written::default();
    let (base_x, base_z) = (at.x * 16, at.z * 16);
    let min_y = chunk.min_y();
    for y in min_y..min_y + chunk.height() as i32 {
        for z in 0..16 {
            for x in 0..16 {
                written
                    .blocks
                    .push(chunk.get(x, y, z).map_or(0, |state: BlockState| state.0));
            }
        }
    }
    for (place, entity) in chunk.block_entities() {
        let at = (
            base_x + i32::from(place.x),
            i32::from(place.y),
            base_z + i32::from(place.z),
        );
        let kind = BLOCK_ENTITY_TYPES[usize::from(entity.kind)].to_owned();
        written.entities.insert(at, (kind, nbt(&entity.data)));
    }
    for tick in chunk.ticks() {
        let what = match tick.of {
            Ticked::Block(block) => BLOCKS[usize::from(block)].name,
            Ticked::Fluid(fluid) => FLUIDS[usize::from(fluid)],
        };
        written.ticks.push((
            (
                base_x + i32::from(tick.at.x),
                i32::from(tick.at.y),
                base_z + i32::from(tick.at.z),
            ),
            what.to_owned(),
            tick.delay,
            i32::from(tick.priority),
        ));
    }
    written.ticks.sort();
    written.marks = chunk
        .marks()
        .iter()
        .map(|mark| {
            (
                base_x + i32::from(mark.x),
                i32::from(mark.y),
                base_z + i32::from(mark.z),
            )
        })
        .collect();
    written.marks.sort_unstable();
    let heights = chunk.heights();
    for (name, map) in [
        ("WORLD_SURFACE", &heights.world_surface),
        ("OCEAN_FLOOR", &heights.ocean_floor),
        ("MOTION_BLOCKING", &heights.motion_blocking),
        (
            "MOTION_BLOCKING_NO_LEAVES",
            &heights.motion_blocking_no_leaves,
        ),
    ] {
        written.heights.push((
            name.to_owned(),
            map.iter()
                .map(|height| min_y + i32::from(*height))
                .collect(),
        ));
    }
    written
}

/// What differs between the game's chunk and Clustine's, in words.
fn differences(at: ChunkPos, game: &Written, ours: &Written) -> Vec<String> {
    let mut found = Vec::new();
    let min_y = -64;
    let differing: Vec<usize> = (0..game.blocks.len().max(ours.blocks.len()))
        .filter(|index| game.blocks.get(*index) != ours.blocks.get(*index))
        .collect();
    if !differing.is_empty() {
        let first = differing[0];
        let (y, z, x) = (first / 256, first / 16 % 16, first % 16);
        let name = |id: Option<&u16>| {
            id.and_then(|id| BlockState(*id).block())
                .map_or("?", |block| block.name)
        };
        found.push(format!(
            "{at:?}: {} blocks differ, the first at ({}, {}, {}): the game has {} and Clustine {}",
            differing.len(),
            at.x * 16 + x as i32,
            min_y + y as i32,
            at.z * 16 + z as i32,
            name(game.blocks.get(first)),
            name(ours.blocks.get(first))
        ));
    }
    // A block with an entity that a run placed and nothing asked about is kept by the
    // game, until the chunk is whole, as a placeholder of its position and the id
    // `DUMMY` (`WorldGenRegion.setBlock`): Clustine's chunk has the entity there,
    // without data.
    let mut ours_entities = ours.entities.clone();
    for (at, (kind, data)) in &game.entities {
        if kind == "DUMMY"
            && data.is_none()
            && let Some(ours) = ours_entities.get_mut(at)
            && ours.1.is_none()
        {
            ours.0 = kind.clone();
        }
    }
    if game.entities != ours_entities {
        found.push(format!(
            "{at:?}: block entities differ: the game has {:?} and Clustine {:?}",
            game.entities, ours.entities
        ));
    }
    if game.ticks != ours.ticks {
        found.push(format!(
            "{at:?}: {} ticks in the game and {} in Clustine differ",
            game.ticks.len(),
            ours.ticks.len()
        ));
    }
    if game.marks != ours.marks {
        // What stands at a mark tells which feature or rule made it.
        let block = |(x, y, z): (i32, i32, i32)| {
            let index = ((y - min_y) * 256 + (z - at.z * 16) * 16 + (x - at.x * 16)) as usize;
            game.blocks
                .get(index)
                .and_then(|id| BlockState(*id).block())
                .map_or("?", |block| block.name)
        };
        let only = |these: &[(i32, i32, i32)], those: &[(i32, i32, i32)]| {
            these
                .iter()
                .filter(|mark| those.binary_search(mark).is_err())
                .take(4)
                .map(|mark| {
                    format!(
                        "{mark:?} on {} above {}",
                        block(*mark),
                        block((mark.0, mark.1 - 1, mark.2))
                    )
                })
                .collect::<Vec<_>>()
        };
        found.push(format!(
            "{at:?}: {} marks in the game and {} in Clustine differ; only the game's: {:?}; only Clustine's: {:?}",
            game.marks.len(),
            ours.marks.len(),
            only(&game.marks, &ours.marks),
            only(&ours.marks, &game.marks)
        ));
    }
    if game.heights != ours.heights {
        found.push(format!("{at:?}: heightmaps differ"));
    }
    found
}

#[test]
#[ignore = "runs the official server; needs a JDK 25, the jar and the EULA accepted"]
fn features_are_placed_as_the_games_own_server_places_them_in_clustines_order() {
    assert_eq!(
        std::env::var("CLUSTINE_ACCEPT_MINECRAFT_EULA").as_deref(),
        Ok("true"),
        "the official server runs only where its EULA is accepted"
    );
    let seed: i64 = std::env::var("CLUSTINE_GAME_FEATURES_SEED")
        .map_or(13579, |seed| seed.parse().expect("a seed is a number"));
    let area = std::env::var("CLUSTINE_GAME_FEATURES_AREA")
        .unwrap_or_else(|_| "100 100 123 123".to_owned());
    let corners: Vec<i32> = area
        .split_whitespace()
        .map(|part| part.parse().expect("an area is four numbers"))
        .collect();
    let area = Area {
        min: ChunkPos::new(corners[0], corners[1]),
        max: ChunkPos::new(corners[2], corners[3]),
    };

    let work = tempfile::Builder::new()
        .prefix("game-features")
        .tempdir()
        .unwrap();
    let pack_directory = work.path().join("pack");
    let turned_off = pack(&pack_directory);
    let classes = work.path().join("classes");
    let javac = Command::new("javac")
        .args(["-proc:none", "-cp", &classpath(), "-d"])
        .arg(&classes)
        .arg(root().join("services/worldgen/tests/java/GameFeatures.java"))
        .status()
        .expect("javac is on the path");
    assert!(javac.success(), "GameFeatures.java did not compile");

    let ascending = game(seed, area, true, &classes, &pack_directory);
    let descending = game(seed, area, false, &classes, &pack_directory);
    assert!(
        ascending == descending,
        "the game's chunks depend on the order within a rank, which ADR-0021 says they cannot"
    );
    let game = parse(&ascending);

    // The game's own buildings, from the jar `cargo datagen` downloaded, as a server
    // reads them from its operator's.
    let templates =
        clustine_worldgen::templates::read_jar(&root().join("target/datagen/26.3/server.jar"))
            .expect("the templates of the jar are read");
    let stages = FeatureStages::with_templates(
        Arc::new(TerrainGenerator::without_structures(seed)),
        Arc::new(templates),
    );
    let scheduler = Scheduler::new(stages, Options::for_processors(num_threads()));
    let compared: Vec<ChunkPos> = area
        .chunks()
        .filter(|e| area.holds_the_runs_of(*e))
        .collect();
    assert!(
        !compared.is_empty(),
        "the area is too small to compare a chunk"
    );
    let mut found = Vec::new();
    for at in &compared {
        let chunk = scheduler.finished(*at).expect("Clustine makes the chunk");
        found.extend(differences(*at, &game[at], &ours(&chunk, *at)));
    }
    let mut report = String::new();
    writeln!(
        report,
        "seed {seed}, area {:?} to {:?}: {} configured features turned off as not built, {} chunks compared",
        area.min,
        area.max,
        turned_off,
        compared.len()
    )
    .unwrap();
    for line in &found {
        writeln!(report, "  {line}").unwrap();
    }
    print!("{report}");
    std::io::stdout().flush().unwrap();
    assert!(found.is_empty(), "{} differences", found.len());
}

fn num_threads() -> usize {
    std::thread::available_parallelism().map_or(4, usize::from)
}

/// The chooser of the judge's areas (the plan, section 3): for every biome of the
/// overworld, a 24 by 24 area around the first chunk outwards from the spawn, at least
/// forty chunks from it, whose middle column has that biome at some height, so that
/// cave biomes are found as well. One line each, `biome x0 z0 x1 z1`, for
/// `CLUSTINE_GAME_FEATURES_AREA`.
#[test]
#[ignore = "a search over the world; run by name with --release --ignored --nocapture"]
fn areas_where_every_biome_of_the_overworld_stands() {
    use clustine_terrain::biome::{BiomeSource, Dimension};
    use clustine_world::ChunkGenerator as _;

    let seed: i64 = std::env::var("CLUSTINE_GAME_FEATURES_SEED")
        .map_or(13579, |seed| seed.parse().expect("a seed is a number"));
    let names = &clustine_data::SYNCED_REGISTRIES
        .iter()
        .find(|registry| registry.name == "minecraft:worldgen/biome")
        .expect("the biomes are a synchronised registry")
        .entries;
    let source = BiomeSource::new(seed, Dimension::Overworld);
    let wanted: Vec<u16> = source
        .possible_biomes()
        .iter()
        .map(|biome| biome.0)
        .collect();
    let spawn = TerrainGenerator::new(seed).spawn().chunk();
    let mut found: BTreeMap<u16, ChunkPos> = BTreeMap::new();
    const STEP: i32 = 6;
    'rings: for ring in (40 / STEP)..(1200 / STEP) {
        for dx in -ring..=ring {
            for dz in -ring..=ring {
                if dx.abs() != ring && dz.abs() != ring {
                    continue;
                }
                let chunk = ChunkPos::new(spawn.x + dx * STEP, spawn.z + dz * STEP);
                for biome in source.column(chunk.x * 4 + 2, chunk.z * 4 + 2) {
                    found.entry(biome.0).or_insert(chunk);
                }
                if wanted.iter().all(|biome| found.contains_key(biome)) {
                    break 'rings;
                }
            }
        }
    }
    for biome in &wanted {
        match found.get(biome) {
            Some(chunk) => println!(
                "{} {} {} {} {}",
                names[usize::from(*biome)],
                chunk.x - 12,
                chunk.z - 12,
                chunk.x + 11,
                chunk.z + 11
            ),
            None => println!("{} not found", names[usize::from(*biome)]),
        }
    }
}
