//! The structure starts and references against the game's own classes: what
//! `crates/clustine-terrain/tests/java/GameStructures.java` wrote, start by start.
//!
//! That program has the generator of the pinned server jar do its "structure
//! starts" and "structure references" steps for areas of chunks, with no server (its
//! head says how, and what it builds by hand). Nothing it writes is in the
//! repository; this test takes a directory from `CLUSTINE_GAME_STRUCTURES` and reads
//! every `.txt` file in it, each of one seed and one dimension.
//!
//! ```text
//! # With <classes> the jars `cargo datagen` unpacked (target/datagen/26.3/extract/classpath,
//! # every .jar under it); a list has lines `area <x0> <z0> <x1> <z1>` and `chunk <x> <z>`;
//! # run from a directory of its own (the game writes `logs/latest.log` where it is run):
//! javac -proc:none -cp <classes> -d <out> crates/clustine-terrain/tests/java/GameStructures.java
//! java -cp <out>:<classes> GameStructures <seed> <dimension> <list> <directory>/<name>.txt
//!
//! CLUSTINE_GAME_STRUCTURES=<directory> \
//!     cargo test --release -p clustine-structures --test game_structures -- --ignored --nocapture
//! ```
//!
//! **The judge.** For every chunk of every area: the same structures start there,
//! and each start has the same pieces in the same order, each with the game's type,
//! box, generation depth and orientation; a piece of a jigsaw structure also with its
//! element, its position, its rotation, its ground level and its junctions, and a
//! piece that places a template with the template, its position, its rotation and
//! its mirror. Then the references of every chunk: the same starts, by structure and
//! chunk. The registry's order of structures, the dimension's sets and the ring
//! positions are compared first.
//!
//! `CLUSTINE_GAME_STRUCTURES_ONLY` narrows the comparison to structures whose name
//! contains a text, for work on one kind.
//!
//! **The lists** are made by [`lists_of_chunks_with_starts_of_every_kind`]: for each
//! of three seeds and each dimension an area around the origin (145 by 145 chunks of
//! the overworld of seed 13579, 81 by 81 of its Nether and its End, 41 by 41 on the
//! other seeds, and for the End a second area far out, where the cities stand), and
//! the start chunks of every structure set nearest the origin until each structure
//! of the set has started 40 times (12 on the other seeds), with as many start
//! chunks where none found a place. The game is asked for every one of them.
//!
//! **What was run**, on 2026-10-14 on the owner's machine (release build; the game
//! took two minutes for the longest list, the judge under a minute for all), and
//! what it gave: **58,864 chunks, 4,006 starts of the game's, every one equal**, of
//! all 52 structures of the registry: between 38 and 73 of each of the 18 abandoned
//! camps, 70 ancient cities, 85 bastions, 75 buried treasures, 70 desert pyramids,
//! 73 end cities, 94 fortresses, 64 igloos, 85 jungle temples, 64 mansions (each of
//! some 580 pieces), 374 mineshafts of both kinds, 64 monuments, 203 nether fossils,
//! 161 ocean ruins, 64 pillager outposts, 505 ruined portals of the seven kinds, 152
//! shipwrecks, 67 strongholds, 64 swamp huts, 77 trail ruins, 134 trial chambers and
//! 339 villages of the five kinds; no start here that the game has not; and all
//! 18,802 references. The 128 ring positions of the strongholds are the game's on
//! each seed.
//!
//! **What the judge does not see.** Of a piece that is built from code (a mineshaft's
//! corridor, a stronghold's room, a monument's) it compares the type, the box, the
//! depth and the orientation, and not what else the game saves of it (a corridor's
//! rails and spiders, a room's doors): the program writes that, in the piece's tag,
//! and the step that places blocks has to compare it. And one term was never other
//! than zero: where two pieces that both follow the terrain are joined, the game
//! halves the difference of their jigsaw blocks' heights, and in all 4,006 starts
//! that difference is nothing.
//!
//! **That the judge can fail** was tried on the lists of seed 0 (703 starts, 1,993
//! references), one change at a time: a spread seeded with its salt plus one (562
//! starts missing, 8 the game has not, the references of 1,331 chunks); a template's
//! jigsaw blocks not ordered by their selection priority (18 starts); a feature's
//! jigsaw block given a name, as before 26.3 (129 starts, every camp with trees);
//! the expansion hack left out (72 starts); the pillager outposts thinned by the
//! chunk and not by its sixteenth (11 missing); references sought seven chunks away
//! in place of eight (one chunk's references). Before they were put right, the
//! judge found the mansions a block too high (all 64) and a secret room drawn with
//! one draw in place of two (28 of 64).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use clustine_structures::geometry::{BoundingBox, IVec3};
use clustine_structures::piece::{StructureMirror, StructurePiece, StructurePiecePayload};
use clustine_structures::{Start, Structures};
use clustine_terrain::biome::Dimension;
use clustine_worldgen_data::structure::{PoolElement, Projection};
use clustine_worldgen_data::structures::{STRUCTURE_SETS, STRUCTURES};

/// A tag as the game writes it as text.
#[derive(Clone, Debug, PartialEq)]
enum Tag {
    Number(f64),
    Text(String),
    List(Vec<Tag>),
    Compound(BTreeMap<String, Tag>),
}

impl Tag {
    fn get(&self, key: &str) -> Option<&Tag> {
        match self {
            Tag::Compound(members) => members.get(key),
            _ => None,
        }
    }

    fn int(&self, key: &str) -> Option<i32> {
        match self.get(key)? {
            Tag::Number(number) => Some(*number as i32),
            _ => None,
        }
    }

    fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Tag::Text(text) => Some(text),
            _ => None,
        }
    }
}

struct Snbt<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Snbt<'_> {
    fn peek(&self) -> u8 {
        self.bytes[self.at]
    }

    fn value(&mut self) -> Tag {
        match self.peek() {
            b'{' => {
                self.at += 1;
                let mut members = BTreeMap::new();
                while self.peek() != b'}' {
                    let start = self.at;
                    while self.peek() != b':' {
                        self.at += 1;
                    }
                    let key = String::from_utf8_lossy(&self.bytes[start..self.at])
                        .trim_matches('"')
                        .to_owned();
                    self.at += 1;
                    members.insert(key, self.value());
                    if self.peek() == b',' {
                        self.at += 1;
                    }
                }
                self.at += 1;
                Tag::Compound(members)
            }
            b'[' => {
                self.at += 1;
                // `[I;` and its like: an array of numbers.
                if self.bytes[self.at + 1] == b';' {
                    self.at += 2;
                }
                let mut items = Vec::new();
                while self.peek() != b']' {
                    items.push(self.value());
                    if self.peek() == b',' {
                        self.at += 1;
                    }
                }
                self.at += 1;
                Tag::List(items)
            }
            quote @ (b'"' | b'\'') => {
                self.at += 1;
                let mut text = Vec::new();
                while self.peek() != quote {
                    if self.peek() == b'\\' {
                        self.at += 1;
                    }
                    text.push(self.peek());
                    self.at += 1;
                }
                self.at += 1;
                Tag::Text(String::from_utf8_lossy(&text).into_owned())
            }
            _ => {
                let start = self.at;
                while !matches!(self.peek(), b',' | b'}' | b']') {
                    self.at += 1;
                }
                let word = String::from_utf8_lossy(&self.bytes[start..self.at]);
                let digits =
                    word.trim_end_matches(['b', 'B', 's', 'S', 'l', 'L', 'f', 'F', 'd', 'D']);
                match digits.parse::<f64>() {
                    Ok(number) => Tag::Number(number),
                    Err(_) => Tag::Text(word.into_owned()),
                }
            }
        }
    }
}

fn parse_tag(text: &str) -> Tag {
    // A sentinel so that a tag cut short stops the scan and does not run off.
    let padded = format!("{text},");
    Snbt {
        bytes: padded.as_bytes(),
        at: 0,
    }
    .value()
}

/// A piece as the game wrote it.
#[derive(Clone, Debug)]
struct GamePiece {
    kind: String,
    depth: i32,
    orientation: String,
    rotation: String,
    bounds: [i32; 6],
    tag: Tag,
}

#[derive(Clone, Debug)]
struct GameStart {
    structure: String,
    bounds: [i32; 6],
    pieces: Vec<GamePiece>,
}

#[derive(Default, Clone, Debug)]
struct GameChunk {
    starts: Vec<GameStart>,
    references: BTreeSet<(String, (i32, i32))>,
}

#[derive(Default)]
struct GameFile {
    seed: i64,
    dimension: String,
    structures: Vec<String>,
    sets: Vec<String>,
    rings: BTreeMap<String, Vec<(i32, i32)>>,
    /// The rectangles asked for, both ends included.
    asked: Vec<[i32; 4]>,
    chunks: BTreeMap<(i32, i32), GameChunk>,
    ended: bool,
}

fn pair(text: &str) -> (i32, i32) {
    let (x, z) = text.split_once(',').expect("a pair of numbers");
    (x.parse().expect("a number"), z.parse().expect("a number"))
}

fn six(fields: &[&str]) -> [i32; 6] {
    let mut bounds = [0; 6];
    for (bound, field) in bounds.iter_mut().zip(fields) {
        *bound = field.parse().expect("a number of a box");
    }
    bounds
}

fn read(text: &str) -> GameFile {
    let mut file = GameFile::default();
    let mut current = (0, 0);
    for line in text.lines() {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "game-structures" => assert_eq!(rest, "1", "another version of the program's output"),
            "seed" => file.seed = rest.parse().expect("a seed"),
            "dimension" => file.dimension = rest.to_owned(),
            "structure" => {
                let fields: Vec<&str> = rest.split(' ').collect();
                assert_eq!(fields[0].parse::<usize>().ok(), Some(file.structures.len()));
                file.structures.push(fields[1].to_owned());
            }
            "set" => file
                .sets
                .push(rest.split(' ').next().unwrap_or_default().to_owned()),
            "ring" => {
                let mut fields = rest.split(' ');
                let set = fields.next().unwrap_or_default().to_owned();
                file.rings.insert(set, fields.map(pair).collect());
            }
            "asked" => {
                let fields: Vec<&str> = rest.split(' ').collect();
                let number = |index: usize| fields[index].parse::<i32>().expect("a chunk");
                file.asked.push(if fields[0] == "area" {
                    [number(1), number(2), number(3), number(4)]
                } else {
                    [number(1), number(2), number(1), number(2)]
                });
            }
            "chunk" => {
                let fields: Vec<&str> = rest.split(' ').collect();
                current = (fields[0].parse().expect("x"), fields[1].parse().expect("z"));
                file.chunks.entry(current).or_default();
            }
            "start" => {
                let fields: Vec<&str> = rest.split(' ').collect();
                file.chunks
                    .entry(current)
                    .or_default()
                    .starts
                    .push(GameStart {
                        structure: fields[0].to_owned(),
                        bounds: six(&fields[2..8]),
                        pieces: Vec::new(),
                    });
            }
            "piece" => {
                let fields: Vec<&str> = rest.splitn(12, ' ').collect();
                let piece = GamePiece {
                    kind: fields[0].to_owned(),
                    depth: fields[1].parse().expect("a depth"),
                    orientation: fields[2].to_owned(),
                    rotation: fields[3].to_owned(),
                    bounds: six(&fields[5..11]),
                    tag: parse_tag(fields[11]),
                };
                let chunk = file.chunks.entry(current).or_default();
                chunk
                    .starts
                    .last_mut()
                    .expect("a piece follows a start")
                    .pieces
                    .push(piece);
            }
            "refs" => {
                let mut fields = rest.split(' ');
                let structure = fields.next().unwrap_or_default().to_owned();
                let chunk = file.chunks.entry(current).or_default();
                for from in fields {
                    chunk.references.insert((structure.clone(), pair(from)));
                }
            }
            "end" => file.ended = true,
            // The terrain's lines are another test's.
            _ => {}
        }
    }
    assert!(file.ended, "the program's output was cut short");
    file
}

fn bounds(bounding_box: &BoundingBox) -> [i32; 6] {
    [
        bounding_box.min_x(),
        bounding_box.min_y(),
        bounding_box.min_z(),
        bounding_box.max_x(),
        bounding_box.max_y(),
        bounding_box.max_z(),
    ]
}

fn projection(projection: Projection) -> &'static str {
    match projection {
        Projection::Rigid => "rigid",
        Projection::TerrainMatching => "terrain_matching",
    }
}

/// An element as the game's tag describes one, to compare the two.
fn element_text(element: &PoolElement) -> String {
    let templates = clustine_worldgen_data::templates();
    match element {
        PoolElement::Empty => "empty".to_owned(),
        PoolElement::Single {
            template,
            projection: of,
            legacy,
            ..
        } => format!(
            "{} minecraft:{} {}",
            if *legacy { "legacy_single" } else { "single" },
            templates.get(usize::from(*template)).name,
            projection(*of)
        ),
        PoolElement::Feature {
            feature,
            projection: of,
        } => {
            format!("feature {feature} {}", projection(*of))
        }
        PoolElement::List {
            elements,
            projection: of,
        } => format!(
            "list [{}] {}",
            elements
                .iter()
                .map(element_text)
                .collect::<Vec<_>>()
                .join("; "),
            projection(*of)
        ),
    }
}

fn game_element_text(tag: &Tag) -> String {
    let of = tag.text("projection").unwrap_or("?");
    match tag.text("element_type").unwrap_or("?") {
        "minecraft:empty_pool_element" => "empty".to_owned(),
        "minecraft:legacy_single_pool_element" => {
            format!("legacy_single {} {of}", tag.text("location").unwrap_or("?"))
        }
        "minecraft:single_pool_element" => {
            format!("single {} {of}", tag.text("location").unwrap_or("?"))
        }
        "minecraft:feature_pool_element" => {
            format!("feature {} {of}", tag.text("feature").unwrap_or("?"))
        }
        "minecraft:list_pool_element" => {
            let inner = match tag.get("elements") {
                Some(Tag::List(elements)) => elements
                    .iter()
                    .map(game_element_text)
                    .collect::<Vec<_>>()
                    .join("; "),
                _ => "?".to_owned(),
            };
            format!("list [{inner}] {of}")
        }
        other => other.to_owned(),
    }
}

/// The game's names of a rotation in a piece's tag.
fn rotation_tag(text: &str) -> &'static str {
    match text {
        "NONE" => "none",
        "CLOCKWISE_90" => "clockwise_90",
        "CLOCKWISE_180" => "180",
        "COUNTERCLOCKWISE_90" => "counterclockwise_90",
        _ => "?",
    }
}

/// What differs between a piece of ours and the game's, or nothing.
fn compare_piece(ours: &StructurePiece, game: &GamePiece) -> Option<String> {
    if ours.piece_type.to_string() != game.kind {
        return Some(format!("type {} for {}", ours.piece_type, game.kind));
    }
    if bounds(&ours.bounding_box) != game.bounds {
        return Some(format!(
            "box {:?} for {:?}",
            bounds(&ours.bounding_box),
            game.bounds
        ));
    }
    if ours.gen_depth != game.depth {
        return Some(format!("depth {} for {}", ours.gen_depth, game.depth));
    }
    let orientation = ours.orientation.map_or("-", |direction| direction.as_str());
    if orientation != game.orientation {
        return Some(format!(
            "orientation {orientation} for {}",
            game.orientation
        ));
    }
    match &ours.payload {
        StructurePiecePayload::Jigsaw(data) => {
            let element = element_text(data.pool_element);
            let theirs = game
                .tag
                .get("pool_element")
                .map(game_element_text)
                .unwrap_or_default();
            if element != theirs {
                return Some(format!("element {element} for {theirs}"));
            }
            let position = (
                game.tag.int("PosX").unwrap_or(i32::MIN),
                game.tag.int("PosY").unwrap_or(i32::MIN),
                game.tag.int("PosZ").unwrap_or(i32::MIN),
            );
            if data.position != IVec3::from(position) {
                return Some(format!("position {:?} for {position:?}", data.position));
            }
            if data.rotation.as_str() != game.rotation {
                return Some(format!(
                    "rotation {} for {}",
                    data.rotation.as_str(),
                    game.rotation
                ));
            }
            let delta = game.tag.int("ground_level_delta").unwrap_or(i32::MIN);
            if ours.ground_level_delta != delta {
                return Some(format!(
                    "ground level {} for {delta}",
                    ours.ground_level_delta
                ));
            }
            let mine: Vec<(i32, i32, i32, i32, &str)> = ours
                .junctions
                .iter()
                .map(|junction| {
                    (
                        junction.source_pos.x,
                        junction.source_pos.y,
                        junction.source_pos.z,
                        junction.delta_y,
                        projection(junction.dest_projection),
                    )
                })
                .collect();
            let theirs: Vec<(i32, i32, i32, i32, &str)> = match game.tag.get("junctions") {
                Some(Tag::List(junctions)) => junctions
                    .iter()
                    .map(|junction| {
                        (
                            junction.int("source_x").unwrap_or(i32::MIN),
                            junction.int("source_ground_y").unwrap_or(i32::MIN),
                            junction.int("source_z").unwrap_or(i32::MIN),
                            junction.int("delta_y").unwrap_or(i32::MIN),
                            junction.text("dest_proj").unwrap_or("?"),
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            };
            if mine != theirs {
                return Some(format!("junctions {mine:?} for {theirs:?}"));
            }
        }
        StructurePiecePayload::Template(data) => {
            let template = game.tag.text("Template").unwrap_or("?");
            // Some kinds save the template's name without its directory.
            let full = data.template_id.to_string();
            if full != template && !full.ends_with(&format!("/{template}")) {
                return Some(format!("template {} for {template}", data.template_id));
            }
            let position = (
                game.tag.int("TPX").unwrap_or(i32::MIN),
                game.tag.int("TPY").unwrap_or(i32::MIN),
                game.tag.int("TPZ").unwrap_or(i32::MIN),
            );
            if data.template_position != IVec3::from(position) {
                return Some(format!(
                    "template at {:?} for {position:?}",
                    data.template_position
                ));
            }
            if data.rotation.as_str() != game.rotation {
                return Some(format!(
                    "rotation {} for {}",
                    data.rotation.as_str(),
                    game.rotation
                ));
            }
            if let Some(saved) = game.tag.text("Rot").or(game.tag.text("Rotation"))
                && data.rotation.as_str() != rotation_tag(saved)
            {
                return Some(format!(
                    "saved rotation {} for {saved}",
                    data.rotation.as_str()
                ));
            }
            let mirror = match data.mirror {
                StructureMirror::None => "none",
                StructureMirror::FrontBack => "front_back",
                StructureMirror::LeftRight => "left_right",
            };
            // The mirror of a template is in the piece's tag where the kind saves
            // one; what the piece itself answers is its orientation's.
            if let Some(saved) = game.tag.text("Mirror")
                && mirror != saved.to_lowercase()
            {
                return Some(format!("mirror {mirror} for {saved}"));
            }
        }
        StructurePiecePayload::Procedural(_) => {}
    }
    None
}

/// What differs between a start of ours and the game's, or nothing.
fn compare_start(ours: &Start, game: &GameStart) -> Option<String> {
    for (index, (mine, theirs)) in ours.pieces.iter().zip(&game.pieces).enumerate() {
        if let Some(difference) = compare_piece(mine, theirs) {
            return Some(format!(
                "piece {index} of {} ({}): {difference}",
                game.pieces.len(),
                theirs.kind
            ));
        }
    }
    if ours.pieces.len() != game.pieces.len() {
        return Some(format!(
            "{} pieces for {}",
            ours.pieces.len(),
            game.pieces.len()
        ));
    }
    if bounds(&ours.bounding_box) != game.bounds {
        return Some(format!(
            "box {:?} for {:?}",
            bounds(&ours.bounding_box),
            game.bounds
        ));
    }
    None
}

#[derive(Default, Clone, Copy)]
struct Counts {
    /// Starts the game has.
    game: usize,
    equal: usize,
    different: usize,
    /// Starts the game has and this crate has not.
    missing: usize,
    /// Starts this crate has and the game has not.
    extra: usize,
    references: usize,
    references_different: usize,
}

fn directory() -> Option<PathBuf> {
    std::env::var_os("CLUSTINE_GAME_STRUCTURES").map(PathBuf::from)
}

#[test]
#[ignore = "needs what GameStructures.java wrote, in CLUSTINE_GAME_STRUCTURES"]
fn structures_start_as_the_games_own_classes_start_them() {
    let directory = directory().expect("CLUSTINE_GAME_STRUCTURES names a directory");
    let only = std::env::var("CLUSTINE_GAME_STRUCTURES_ONLY").unwrap_or_default();
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

    let mut totals: BTreeMap<String, Counts> = BTreeMap::new();
    let mut first: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut chunks_judged = 0;
    for path in &paths {
        let file = read(&std::fs::read_to_string(path).expect("the file can be read"));
        let dimension = match file.dimension.as_str() {
            "overworld" => Dimension::Overworld,
            "nether" => Dimension::Nether,
            "end" => Dimension::End,
            other => panic!("no dimension {other}"),
        };
        let started = std::time::Instant::now();
        let structures = Structures::new(file.seed, dimension);

        // The registry's order, the dimension's sets and the rings.
        let names: Vec<&str> = STRUCTURES.iter().map(|structure| structure.name).collect();
        assert_eq!(
            file.structures, names,
            "the game holds its structures in another order"
        );
        let sets: Vec<&str> = structures
            .possible_sets()
            .iter()
            .map(|set| STRUCTURE_SETS[*set].name)
            .collect();
        assert_eq!(
            file.sets, sets,
            "the game has other sets in the {}",
            file.dimension
        );
        for (set, positions) in &file.rings {
            let index = STRUCTURE_SETS
                .iter()
                .position(|known| known.name == set)
                .expect("a set");
            assert_eq!(structures.rings()[index], *positions, "the rings of {set}");
        }

        let wanted = |name: &str| only.is_empty() || name.contains(&only);
        let mut ours: BTreeMap<(i32, i32), Vec<Start>> = BTreeMap::new();
        let mut counts: BTreeMap<String, Counts> = BTreeMap::new();
        let nothing = GameChunk::default();
        for [x0, z0, x1, z1] in &file.asked {
            for x in x0 - 8..=x1 + 8 {
                for z in z0 - 8..=z1 + 8 {
                    ours.entry((x, z))
                        .or_insert_with(|| structures.starts(x, z));
                }
            }
            for x in *x0..=*x1 {
                for z in *z0..=*z1 {
                    chunks_judged += 1;
                    let game = file.chunks.get(&(x, z)).unwrap_or(&nothing);
                    let mine = &ours[&(x, z)];
                    for theirs in &game.starts {
                        if !wanted(&theirs.structure) {
                            continue;
                        }
                        let count = counts.entry(theirs.structure.clone()).or_default();
                        count.game += 1;
                        let found = mine
                            .iter()
                            .find(|start| start.data().name == theirs.structure);
                        let difference = match found {
                            None => {
                                count.missing += 1;
                                Some("no start".to_owned())
                            }
                            Some(start) => {
                                let difference = compare_start(start, theirs);
                                if difference.is_some() {
                                    count.different += 1;
                                } else {
                                    count.equal += 1;
                                }
                                difference
                            }
                        };
                        if let Some(difference) = difference {
                            let seen = first.entry(theirs.structure.clone()).or_default();
                            if seen.len() < 3 {
                                seen.push(format!(
                                    "{} {} chunk {x} {z}: {difference}",
                                    file.dimension, file.seed
                                ));
                            }
                        }
                    }
                    for start in mine {
                        let name = start.data().name;
                        if wanted(name)
                            && !game.starts.iter().any(|theirs| theirs.structure == name)
                        {
                            counts.entry(name.to_owned()).or_default().extra += 1;
                            let seen = first.entry(name.to_owned()).or_default();
                            if seen.len() < 3 {
                                seen.push(format!(
                                    "{} {} chunk {x} {z}: a start the game has not",
                                    file.dimension, file.seed
                                ));
                            }
                        }
                    }

                    // The references, structure by structure.
                    let referred: BTreeSet<(String, (i32, i32))> =
                        Structures::references(x, z, |x, z| ours[&(x, z)].as_slice())
                            .into_iter()
                            .map(|(structure, from)| (STRUCTURES[structure].name.to_owned(), from))
                            .collect();
                    let mut names: BTreeSet<&String> =
                        referred.iter().map(|(name, _)| name).collect();
                    names.extend(game.references.iter().map(|(name, _)| name));
                    for name in names {
                        if !wanted(name) {
                            continue;
                        }
                        let mine: Vec<_> = referred.iter().filter(|(of, _)| of == name).collect();
                        let theirs: Vec<_> = game
                            .references
                            .iter()
                            .filter(|(of, _)| of == name)
                            .collect();
                        let count = counts.entry(name.clone()).or_default();
                        count.references += theirs.len();
                        if mine != theirs {
                            count.references_different += 1;
                        }
                    }
                }
            }
        }

        let mut report = String::new();
        for (name, count) in &counts {
            let total = totals.entry(name.clone()).or_default();
            total.game += count.game;
            total.equal += count.equal;
            total.different += count.different;
            total.missing += count.missing;
            total.extra += count.extra;
            total.references += count.references;
            total.references_different += count.references_different;
            let _ = write!(
                report,
                " {}={}",
                name.trim_start_matches("minecraft:"),
                count.game
            );
        }
        println!(
            "{}: {} of seed {} in {:.1?}; starts:{report}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            file.dimension,
            file.seed,
            started.elapsed()
        );
    }

    println!("\n{chunks_judged} chunks judged");
    println!(
        "{:44} {:>6} {:>6} {:>6} {:>6} {:>6} {:>8} {:>6}",
        "structure", "game", "equal", "differ", "miss", "extra", "refs", "differ"
    );
    let mut wrong = 0;
    for (name, count) in &totals {
        println!(
            "{:44} {:>6} {:>6} {:>6} {:>6} {:>6} {:>8} {:>6}",
            name.trim_start_matches("minecraft:"),
            count.game,
            count.equal,
            count.different,
            count.missing,
            count.extra,
            count.references,
            count.references_different
        );
        wrong += count.different + count.missing + count.extra + count.references_different;
    }
    let sum = |of: fn(&Counts) -> usize| totals.values().map(of).sum::<usize>();
    println!(
        "all: {} starts of the game's, {} equal, {} different, {} missing here, {} here and \
         not in the game; {} references, the references of {} chunks and structures differ",
        sum(|count| count.game),
        sum(|count| count.equal),
        sum(|count| count.different),
        sum(|count| count.missing),
        sum(|count| count.extra),
        sum(|count| count.references),
        sum(|count| count.references_different),
    );
    for (name, differences) in &first {
        for difference in differences {
            println!("{name}: {difference}");
        }
    }
    assert_eq!(wrong, 0, "starts or references differ from the game's");
}

#[test]
fn a_tag_written_as_text_reads_as_its_tree() {
    let tag = parse_tag(
        r#"{BB:[I;-272,70,328,-264,78,335],GD:0,Mirror:"FRONT_BACK",O:2,Properties:{cold:0b,mossiness:0.2f},junctions:[{delta_y:-1,dest_proj:"rigid"}],Template:"minecraft:ruined_portal/portal_1"}"#,
    );
    assert_eq!(tag.int("O"), Some(2));
    assert_eq!(tag.text("Mirror"), Some("FRONT_BACK"));
    assert_eq!(
        tag.text("Template"),
        Some("minecraft:ruined_portal/portal_1")
    );
    assert_eq!(
        tag.get("Properties").and_then(|tag| tag.int("cold")),
        Some(0)
    );
    let Some(Tag::List(bounds)) = tag.get("BB") else {
        panic!("a box is a list");
    };
    assert_eq!(bounds.len(), 6);
    assert_eq!(bounds[0], Tag::Number(-272.0));
    let Some(Tag::List(junctions)) = tag.get("junctions") else {
        panic!("junctions are a list");
    };
    assert_eq!(junctions[0].int("delta_y"), Some(-1));
    assert_eq!(junctions[0].text("dest_proj"), Some("rigid"));
}

/// The seeds the lists are made for: the first with large areas and many chunks of
/// every kind, the others with smaller ones.
const SEEDS: [i64; 3] = [13579, 0, -7_046_029_254_386_353_131];

/// Writes the lists of chunks for `GameStructures.java` into the directory
/// `CLUSTINE_GAME_STRUCTURES_LISTS`: for each seed and dimension an area, and the
/// start chunks of every structure set nearest the origin until each structure of
/// the set has started often enough here, with as many start chunks where none of
/// the set's structures found a place. The game is asked for all of them, so a
/// start that this crate wrongly leaves out shows as well.
#[test]
#[ignore = "writes the lists for GameStructures.java into CLUSTINE_GAME_STRUCTURES_LISTS"]
fn lists_of_chunks_with_starts_of_every_kind() {
    use clustine_worldgen_data::structure::Spread;
    let directory = PathBuf::from(
        std::env::var_os("CLUSTINE_GAME_STRUCTURES_LISTS").expect("a directory for the lists"),
    );
    std::fs::create_dir_all(&directory).expect("the directory can be made");
    for (number, seed) in SEEDS.into_iter().enumerate() {
        let (wanted, reach) = if number == 0 { (40, 2400) } else { (12, 1200) };
        for (dimension, name) in [
            (Dimension::Overworld, "overworld"),
            (Dimension::Nether, "nether"),
            (Dimension::End, "end"),
        ] {
            let structures = Structures::new(seed, dimension);
            let mut list = String::new();
            let side = match (number, dimension) {
                (0, Dimension::Overworld) => 72,
                (0, _) => 40,
                _ => 20,
            };
            let _ = writeln!(list, "area {} {} {} {}", -side, -side, side, side);
            if dimension == Dimension::End {
                // The cities stand on the outer islands.
                let _ = writeln!(
                    list,
                    "area {} {} {} {}",
                    80,
                    80,
                    80 + 2 * side,
                    80 + 2 * side
                );
            }
            for set in structures.possible_sets() {
                let data = &STRUCTURE_SETS[*set];
                let mut found: BTreeMap<usize, usize> = BTreeMap::new();
                let mut empty = 0;
                let mut candidates: Vec<(i32, i32)> = Vec::new();
                match data.placement.spread {
                    Spread::ConcentricRings { .. } => {
                        candidates.extend(structures.rings()[*set].iter().take(wanted));
                    }
                    Spread::Random { spacing, .. } => {
                        // Squares of the spread in rings around the origin's.
                        let cells = (reach / spacing).clamp(1, 400);
                        for ring in 0..=cells {
                            for x in -ring..=ring {
                                for z in -ring..=ring {
                                    if x.abs().max(z.abs()) == ring {
                                        candidates.push((x * spacing, z * spacing));
                                    }
                                }
                            }
                        }
                    }
                }
                let enough = |found: &BTreeMap<usize, usize>| {
                    data.structures.iter().all(|(structure, _)| {
                        found.get(&usize::from(*structure)).copied() >= Some(wanted)
                    })
                };
                for (x, z) in candidates {
                    if enough(&found) {
                        break;
                    }
                    let (x, z) = match data.placement.spread {
                        Spread::Random {
                            spacing,
                            separation,
                            spread_type,
                        } => clustine_structures::placement::potential_chunk(
                            seed,
                            data.placement.salt,
                            spacing,
                            separation,
                            spread_type,
                            x,
                            z,
                        ),
                        Spread::ConcentricRings { .. } => (x, z),
                    };
                    if !structures.is_structure_chunk(*set, x, z) {
                        continue;
                    }
                    let starts = structures.starts(x, z);
                    let of_set = starts.iter().find(|start| {
                        data.structures
                            .iter()
                            .any(|(structure, _)| usize::from(*structure) == start.structure)
                    });
                    match of_set {
                        Some(start) => {
                            let count = found.entry(start.structure).or_default();
                            if *count < wanted {
                                *count += 1;
                                let _ = writeln!(list, "chunk {x} {z}");
                            }
                        }
                        None if empty < wanted => {
                            empty += 1;
                            let _ = writeln!(list, "chunk {x} {z}");
                        }
                        None => {}
                    }
                }
                let counts: Vec<String> = data
                    .structures
                    .iter()
                    .map(|(structure, _)| {
                        format!(
                            "{}={}",
                            STRUCTURES[usize::from(*structure)]
                                .name
                                .trim_start_matches("minecraft:"),
                            found.get(&usize::from(*structure)).copied().unwrap_or(0)
                        )
                    })
                    .collect();
                println!(
                    "{name} {seed} {}: {} and {empty} without",
                    data.name,
                    counts.join(" ")
                );
            }
            std::fs::write(directory.join(format!("{name}_{seed}.txt")), list)
                .expect("the list can be written");
        }
    }
}
