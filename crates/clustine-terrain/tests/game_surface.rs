//! The surface against the game's own classes: what `tests/java/GameSurface.java`
//! wrote, block by block.
//!
//! That program has the generator of the pinned server jar do its "fill from noise"
//! step and then its "build surface" step for a list of chunks and nothing after
//! them, with no server (its head says how, and what it builds by hand). Nothing it
//! writes is in the repository; this test takes a directory from
//! `CLUSTINE_GAME_SURFACE` and reads every `.txt` file in it, each of one seed and one
//! dimension.
//!
//! ```text
//! # The lists of chunks, which this file makes (the same every time):
//! CLUSTINE_GAME_SURFACE_LISTS=<lists> \
//!     cargo test --release -p clustine-terrain --test game_surface -- --ignored --nocapture lists
//!
//! # With <classes> the jars `cargo datagen` unpacked (target/datagen/26.3/extract/classpath,
//! # every .jar under it); for each list `<dimension>_<seed>.txt`, with the number of
//! # chunks at the head of the list that the gate is written for (16 here):
//! javac -proc:none -cp <classes> -d <out> crates/clustine-terrain/tests/java/GameSurface.java
//! java -cp <out>:<classes> GameSurface <seed> <dimension> <lists>/<dimension>_<seed>.txt 16 \
//!     <directory>/<dimension>_<seed>.txt
//!
//! CLUSTINE_GAME_SURFACE=<directory> \
//!     cargo test --release -p clustine-terrain --test game_surface -- --ignored --nocapture game
//! ```
//!
//! **The judge** ([`chunks_have_the_surface_the_games_own_classes_give_them`]): for
//! every chunk, every block after the surface equal, and the positions marked for
//! post-processing equal. The surface is made **with the biomes the program read**
//! (the chunk's cells and a ring around them), because a biome that is an exact tie
//! is not always the game's here (`clustine_terrain::biome`). Beside that the test
//! makes this crate's own biomes and counts the cells at which they are not the
//! program's, the chunks where that changes a block, and those blocks: they are
//! printed and judge nothing.
//!
//! **The gate**, for the chunks at the head of each list, so that a differing block
//! can be traced: the fill's blocks by their hash (every chunk); the bands of
//! terracotta of the world; the router's `chunk_surface_level` at every column as the
//! rules sample it, bit for bit; the depth of the surface and its second noise at
//! every column; the biome at 30 blocks of every column through the game's
//! `BiomeManager`; whether it is cold enough to snow at four heights of every column
//! for three biomes; a band of terracotta at every column; and for the overworld the
//! hashes of the ore veins' three density functions sampled as the rule samples them
//! and of the fourth at single blocks. For a block that differs the test prints the
//! biome there and what this crate's rule was asked and answered
//! (`Terrain::build_surface_watching`); the game's side of that cannot be had from
//! outside its classes.
//!
//! **What was run**, on 2026-10-13 on the owner's machine (release build), and what
//! it gave. Seed 13579: 2,400 chunks of the overworld (235,929,600 blocks, of which
//! the step changed 42,345,687; 100,808 marks), 1,000 of the Nether (32,768,000
//! blocks, 2,649,073 changed, 243 marks) and 1,000 of the End (which the step leaves
//! as they are). Seeds 0 and -7046029254386353131: 300 chunks of the overworld and
//! 150 of each of the other two, each. The lists hold the origin, chunks a million
//! and ten million blocks out and at the world's edge, forty chunks looked for by
//! each of forty biomes of the overworld (the three badlands, the two frozen oceans,
//! desert, the beaches and shores, the peaks, slopes and grove, the two swamps, the
//! three windswept biomes, the old growth taigas, ice spikes, mushroom fields, the
//! dappled forest, the oceans of every temperature, rivers, plains, the deep dark and
//! the three kinds of caves) and by each of the Nether's five and the End's four,
//! squares of four by four, and chunks anywhere within 48,000 blocks. **Every one of
//! 380,108,800 blocks was equal**, and so were all 124,156 marks, every fill, the
//! bands of all three worlds and every one of the gate's 1,450,176 values of 144
//! chunks. Of the overworld's chunks of seed 13579, 137 hold white terracotta, 116
//! packed ice, 223 snow blocks, 194 copper ore and 273 deepslate iron ore (83 and 124
//! a block of the raw ore), 119 cinnabar, 39 mycelium, 57 mud, 27 powder snow and 4
//! calcite.
//!
//! With this crate's own biomes in place of the program's, 3,279 of 10,368,000 cells
//! of the overworld and 176 of 2,995,200 of the Nether are another biome, every one of
//! them an exact tie; that changes 923 blocks in 15 chunks of the overworld and 391 in
//! 3 of the Nether.
//!
//! **That the judge can fail** was tried on the 300 chunks of the overworld of seed
//! 0, one change at a time: the slope taken as steep from 5 in place of 4 (13 blocks
//! in 2 chunks differed, snow for packed ice on the frozen peaks); an iceberg's
//! chance under water 0.16 in place of 0.15 (290 blocks in 16 chunks); the cells'
//! offsets 0.91 of a cell wide in place of 0.9 (110 blocks in 32 chunks, and 44
//! values of the gate's biomes); an ore vein's chance a hundredth higher (190 blocks
//! in 47 chunks); and the rule's fluids not marked (the marks of 5 chunks and more).

use std::collections::BTreeMap;
use std::path::Path;

use clustine_data::{BLOCKS, BlockState, synced_registry};
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::Terrain;
use clustine_terrain::noise_chunk::{Scratch, Volume};
use clustine_terrain::surface::Biomes;
use clustine_world::{Biome, InChunk};

/// How many chunks at the head of a list the gate is written for.
const GATE: usize = 16;

/// The seeds: the one every fixture of the plan has, zero, and one with the top bit
/// set.
const SEEDS: [i64; 3] = [13579, 0, -7_046_029_254_386_353_131];

/// What the program wrote for a chunk of the gate.
struct Gate {
    level: Vec<u32>,
    depth: Vec<i32>,
    secondary: Vec<u64>,
    /// The first height, the step and the count, and the biomes by our numbers.
    zoom: (i32, i32, usize, Vec<Biome>),
    /// Three biomes, and for each the answers for every column at four heights.
    cold: (Vec<Biome>, Vec<bool>),
    band: Vec<BlockState>,
    /// The height of the volume, and the four hashes.
    veins: Option<(usize, Vec<u64>)>,
}

/// What the program wrote for one chunk.
struct Written {
    x: i32,
    z: i32,
    /// The 36 columns of cells, by our numbers.
    biomes: Vec<Vec<Biome>>,
    filled_hash: u64,
    /// After the surface, in the volume's order.
    blocks: Vec<BlockState>,
    marks: Vec<InChunk>,
    gate: Option<Gate>,
}

/// What the program wrote for one seed and dimension.
struct File {
    seed: i64,
    dimension: Dimension,
    min_y: i32,
    height: i32,
    bands: Vec<BlockState>,
    chunks: Vec<Written>,
}

fn runs(fields: &[&str]) -> Vec<BlockState> {
    let mut blocks = Vec::new();
    for run in fields {
        let (state, count) = run.split_once('*').expect("a run is a state and a count");
        let state = BlockState(state.parse().unwrap());
        blocks.extend(std::iter::repeat_n(state, count.parse().unwrap()));
    }
    blocks
}

fn hex<const DIGITS: usize>(text: &str) -> Vec<u64> {
    text.as_bytes()
        .chunks(DIGITS)
        .map(|digits| u64::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap())
        .collect()
}

fn numbers<T: std::str::FromStr>(fields: &[&str]) -> Vec<T>
where
    T::Err: std::fmt::Debug,
{
    fields.iter().map(|field| field.parse().unwrap()).collect()
}

fn read(path: &Path) -> File {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let registry = synced_registry("minecraft:worldgen/biome").unwrap();
    let mut file = File {
        seed: 0,
        dimension: Dimension::Overworld,
        min_y: 0,
        height: 0,
        bands: Vec::new(),
        chunks: Vec::new(),
    };
    // The program's numbers of the biomes, as ours.
    let mut biome_of: Vec<Biome> = Vec::new();
    let mut ended = false;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        match fields[..] {
            ["game-surface", "1"] => {}
            ["seed", seed] => file.seed = seed.parse().unwrap(),
            ["dimension", name] => {
                file.dimension = match name {
                    "overworld" => Dimension::Overworld,
                    "nether" => Dimension::Nether,
                    "end" => Dimension::End,
                    other => panic!("no dimension {other}"),
                }
            }
            ["noise", min_y, height, _, _, _] => {
                file.min_y = min_y.parse().unwrap();
                file.height = height.parse().unwrap();
            }
            ["heights", min_y, height] => {
                let (lowest, sections) = file.dimension.sections();
                assert_eq!(min_y.parse::<i32>().unwrap(), lowest * 16);
                assert_eq!(height.parse::<usize>().unwrap(), sections * 16);
            }
            ["biome", number, name] => {
                assert_eq!(number.parse::<usize>().unwrap(), biome_of.len());
                let ours = registry
                    .id_of(name)
                    .unwrap_or_else(|| panic!("{name} is no biome"));
                biome_of.push(Biome(ours as u16));
            }
            ["end", chunks] => {
                assert_eq!(chunks.parse::<usize>().unwrap(), file.chunks.len());
                ended = true;
            }
            ["chunk", x, z] => file.chunks.push(Written {
                x: x.parse().unwrap(),
                z: z.parse().unwrap(),
                biomes: Vec::new(),
                filled_hash: 0,
                blocks: Vec::new(),
                marks: Vec::new(),
                gate: None,
            }),
            _ => {
                if fields[0] == "bands" {
                    file.bands = numbers::<u16>(&fields[1..])
                        .into_iter()
                        .map(BlockState)
                        .collect();
                    continue;
                }
                let chunk = file
                    .chunks
                    .last_mut()
                    .expect("a chunk's line after a chunk");
                let gate = |chunk: &mut Written| {
                    chunk.gate.get_or_insert_with(|| Gate {
                        level: Vec::new(),
                        depth: Vec::new(),
                        secondary: Vec::new(),
                        zoom: (0, 0, 0, Vec::new()),
                        cold: (Vec::new(), Vec::new()),
                        band: Vec::new(),
                        veins: None,
                    });
                };
                match fields[0] {
                    "biomes" => {
                        chunk.biomes = fields[1..]
                            .iter()
                            .map(|column| {
                                let mut cells = Vec::new();
                                for run in column.split(',') {
                                    let (biome, count) = run.split_once('*').unwrap();
                                    let biome = biome_of[biome.parse::<usize>().unwrap()];
                                    cells
                                        .extend(std::iter::repeat_n(biome, count.parse().unwrap()));
                                }
                                cells
                            })
                            .collect();
                        assert_eq!(chunk.biomes.len(), 36);
                    }
                    "filled" => chunk.filled_hash = u64::from_str_radix(fields[1], 16).unwrap(),
                    "blocks" => chunk.blocks = runs(&fields[1..]),
                    "marks" => {
                        chunk.marks = fields[1..]
                            .iter()
                            .map(|mark| {
                                let at: Vec<i32> =
                                    mark.split(',').map(|part| part.parse().unwrap()).collect();
                                InChunk::new(at[0] as usize, at[1], at[2] as usize)
                            })
                            .collect();
                        chunk.marks.sort_unstable();
                    }
                    "level" => {
                        gate(chunk);
                        chunk.gate.as_mut().unwrap().level = hex::<8>(fields[1])
                            .into_iter()
                            .map(|bits| bits as u32)
                            .collect();
                    }
                    "depth" => {
                        gate(chunk);
                        chunk.gate.as_mut().unwrap().depth = numbers(&fields[1..]);
                    }
                    "secondary" => {
                        gate(chunk);
                        chunk.gate.as_mut().unwrap().secondary = hex::<16>(fields[1]);
                    }
                    "zoom" => {
                        gate(chunk);
                        chunk.gate.as_mut().unwrap().zoom = (
                            fields[1].parse().unwrap(),
                            fields[2].parse().unwrap(),
                            fields[3].parse().unwrap(),
                            numbers::<usize>(&fields[4..])
                                .into_iter()
                                .map(|number| biome_of[number])
                                .collect(),
                        );
                    }
                    "cold" => {
                        gate(chunk);
                        chunk.gate.as_mut().unwrap().cold = (
                            numbers::<usize>(&fields[1..4])
                                .into_iter()
                                .map(|number| biome_of[number])
                                .collect(),
                            fields[4].bytes().map(|bit| bit == b'1').collect(),
                        );
                    }
                    "band" => {
                        gate(chunk);
                        chunk.gate.as_mut().unwrap().band = numbers::<u16>(&fields[1..])
                            .into_iter()
                            .map(BlockState)
                            .collect();
                    }
                    "veins" => {
                        gate(chunk);
                        chunk.gate.as_mut().unwrap().veins = Some((
                            fields[1].parse().unwrap(),
                            fields[2..]
                                .iter()
                                .map(|hash| u64::from_str_radix(hash, 16).unwrap())
                                .collect(),
                        ));
                    }
                    _ => panic!(
                        "the line {:?} is not of the program's",
                        &line[..line.len().min(80)]
                    ),
                }
            }
        }
    }
    assert!(ended, "{} was not written to its end", path.display());
    file
}

/// FNV-1a over bytes.
fn fnv(bytes: impl Iterator<Item = u8>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The blocks of a chunk in the order of the program's volume: y fastest, then x,
/// then z.
fn in_the_volumes_order(blocks: &[BlockState], height: usize) -> Vec<BlockState> {
    let mut ordered = Vec::with_capacity(blocks.len());
    for z in 0..16 {
        for x in 0..16 {
            for y in 0..height {
                ordered.push(blocks[y << 8 | z << 4 | x]);
            }
        }
    }
    ordered
}

fn block_name(state: BlockState) -> &'static str {
    state.block_id().map_or("no block", |id| BLOCKS[id].name)
}

fn biome_name(biome: Biome) -> &'static str {
    synced_registry("minecraft:worldgen/biome").unwrap().entries[usize::from(biome.0)]
        .trim_start_matches("minecraft:")
}

/// What was counted over one file.
#[derive(Default)]
struct Counts {
    chunks: usize,
    blocks: usize,
    blocks_different: usize,
    chunks_with_a_different_block: usize,
    fills_different: usize,
    marks: usize,
    marks_different: usize,
    /// Blocks the surface step changed, by the game's output.
    blocks_changed: usize,
    /// With this crate's own biomes in place of the program's.
    cells: usize,
    cells_with_another_biome: usize,
    cells_that_are_no_tie: usize,
    chunks_with_another_biome: usize,
    chunks_another_biome_changes: usize,
    blocks_another_biome_changes: usize,
    /// The first chunks that have a cell with another biome.
    first_with_another_biome: Vec<(i32, i32)>,
    gate_chunks: usize,
    gate_values: usize,
    gate_values_different: usize,
    bands_different: usize,
    /// Of the blocks after the surface, by block.
    kinds: BTreeMap<&'static str, usize>,
    /// Of the chunks, by the biome at the highest block of their middle column.
    biomes: BTreeMap<&'static str, usize>,
    /// Chunks that hold a block of each of these.
    with: BTreeMap<&'static str, usize>,
    farthest: i64,
}

/// Blocks whose presence says that a part of the step was at work in a chunk.
const TELLING: [&str; 22] = [
    "minecraft:packed_ice",
    "minecraft:snow_block",
    "minecraft:powder_snow",
    "minecraft:ice",
    "minecraft:white_terracotta",
    "minecraft:light_gray_terracotta",
    "minecraft:red_sand",
    "minecraft:copper_ore",
    "minecraft:raw_copper_block",
    "minecraft:granite",
    "minecraft:deepslate_iron_ore",
    "minecraft:raw_iron_block",
    "minecraft:tuff",
    "minecraft:mycelium",
    "minecraft:mud",
    "minecraft:podzol",
    "minecraft:calcite",
    "minecraft:cinnabar",
    "minecraft:sulfur",
    "minecraft:coarse_dirt",
    "minecraft:sandstone",
    "minecraft:soul_sand",
];

fn judge(file: &File, name: &str) -> (Counts, Vec<String>) {
    let terrain = Terrain::new(file.seed, file.dimension);
    let source = BiomeSource::new(file.seed, file.dimension);
    let settings = terrain.settings();
    assert_eq!((settings.min_y, settings.height), (file.min_y, file.height));
    let height = file.height as usize;
    let mut counts = Counts::default();
    let mut findings = Vec::new();
    let mut find = |text: String| {
        if findings.len() < 30 {
            findings.push(text);
        }
    };

    if terrain.bands() != file.bands {
        counts.bands_different += 1;
        find(format!(
            "{name}: the bands of terracotta are not the game's"
        ));
    }

    let mut scratch = Scratch::default();
    for written in &file.chunks {
        counts.chunks += 1;
        counts.farthest = counts
            .farthest
            .max(i64::from(written.x).abs())
            .max(i64::from(written.z).abs());
        let volume: Volume = terrain.volume(written.x, written.z);
        assert_eq!(written.blocks.len(), volume.len());

        let mut columns = written.biomes.iter();
        let biomes = Biomes::from_columns(file.dimension, written.x, written.z, |_, _| {
            columns.next().expect("36 columns").clone()
        });

        // The fill, by its hash.
        let (mut made, _) = terrain.fill_with_density(written.x, written.z, &mut scratch);
        let filled = in_the_volumes_order(&made.blocks, height);
        if fnv(filled.iter().flat_map(|state| state.0.to_le_bytes())) != written.filled_hash {
            counts.fills_different += 1;
            find(format!(
                "chunk {} {}: the fill is not the game's",
                written.x, written.z
            ));
        }

        // The surface.
        let before = made.clone();
        terrain.build_surface(&mut made, written.x, written.z, &biomes, &mut scratch);
        let ours = in_the_volumes_order(&made.blocks, height);
        let mut different = 0;
        for (index, (our, their)) in ours.iter().zip(&written.blocks).enumerate() {
            *counts.kinds.entry(block_name(*their)).or_default() += 1;
            counts.blocks_changed += usize::from(filled[index] != *their);
            if our == their {
                continue;
            }
            different += 1;
            if different <= 2 {
                let y = file.min_y + (index % height) as i32;
                let x = volume.min_x + (index / height % 16) as i32;
                let z = volume.min_z + (index / height / 16) as i32;
                let mut again = before.clone();
                let watched = terrain.build_surface_watching(
                    &mut again,
                    written.x,
                    written.z,
                    &biomes,
                    [x, y, z],
                );
                find(format!(
                    "chunk {} {} block {x} {y} {z} in {}: {} ({}) here, {} ({}) in the game; \
                     after the fill {}; this crate's rule: {watched:?}",
                    written.x,
                    written.z,
                    biome_name(biomes.at_block(terrain.zoom(), x, y, z)),
                    block_name(*our),
                    our.0,
                    block_name(*their),
                    their.0,
                    block_name(filled[index]),
                ));
            }
        }
        counts.blocks += ours.len();
        counts.blocks_different += different;
        counts.chunks_with_a_different_block += usize::from(different != 0);
        for telling in TELLING {
            if written
                .blocks
                .iter()
                .any(|state| block_name(*state) == telling)
            {
                *counts.with.entry(telling).or_default() += 1;
            }
        }
        // The biome at the highest block of the middle column.
        let top = (0..height)
            .rev()
            .find(|y| !written.blocks[y + (8 + 8 * 16) * height].is_air())
            .map_or(file.min_y, |y| file.min_y + y as i32 + 1);
        *counts
            .biomes
            .entry(biome_name(biomes.at_block(
                terrain.zoom(),
                volume.min_x + 8,
                top,
                volume.min_z + 8,
            )))
            .or_default() += 1;

        // The marks.
        counts.marks += written.marks.len();
        if made.marks != written.marks {
            counts.marks_different += 1;
            find(format!(
                "chunk {} {}: {} marks here, {} in the game",
                written.x,
                written.z,
                made.marks.len(),
                written.marks.len()
            ));
        }

        // This crate's own biomes: where they are not the program's, and what that
        // changes.
        let own = Biomes::from_source(&source, written.x, written.z);
        let mut other_cells = 0;
        let mut columns = written.biomes.iter();
        for cell_x in written.x * 4 - 1..=written.x * 4 + 4 {
            for cell_z in written.z * 4 - 1..=written.z * 4 + 4 {
                let theirs = columns.next().expect("36 columns");
                for (up, their) in theirs.iter().enumerate() {
                    counts.cells += 1;
                    let (lowest, _) = file.dimension.sections();
                    let cell_y = lowest * 4 + up as i32;
                    if own.cell(cell_x, cell_y, cell_z) != *their {
                        other_cells += 1;
                        // Another biome than the game's is one that is exactly as
                        // near, or it is a fault.
                        if !source.found(cell_x, cell_y, cell_z).tied.contains(their) {
                            counts.cells_that_are_no_tie += 1;
                            find(format!(
                                "cell {cell_x} {cell_y} {cell_z}: {} in the game, and no tie",
                                biome_name(*their)
                            ));
                        }
                    }
                }
            }
        }
        if other_cells != 0 {
            counts.cells_with_another_biome += other_cells;
            counts.chunks_with_another_biome += 1;
            if counts.first_with_another_biome.len() < 6 {
                counts.first_with_another_biome.push((written.x, written.z));
            }
            let mut with_own = before.clone();
            terrain.build_surface(&mut with_own, written.x, written.z, &own, &mut scratch);
            let changed = with_own
                .blocks
                .iter()
                .zip(&made.blocks)
                .filter(|(own, given)| own != given)
                .count();
            counts.blocks_another_biome_changes += changed;
            counts.chunks_another_biome_changes += usize::from(changed != 0);
        }

        // The gate.
        let Some(gate) = &written.gate else {
            continue;
        };
        counts.gate_chunks += 1;
        let mut differs = |what: &str, at: String, values: usize, different: usize| {
            counts.gate_values += values;
            counts.gate_values_different += different;
            if different != 0 {
                find(format!(
                    "chunk {} {}: {different} of {values} values of {what} are not the \
                     game's; the first: {at}",
                    written.x, written.z
                ));
            }
        };
        let level = terrain.surface_level(written.x, written.z);
        let wrong: Vec<usize> = (0..256)
            .filter(|column| level[*column].to_bits() != gate.level[*column])
            .collect();
        differs(
            "the surface level",
            wrong.first().map_or(String::new(), |column| {
                format!(
                    "column {} {}: {} here, {} in the game",
                    column & 15,
                    column >> 4,
                    level[*column],
                    f32::from_bits(gate.level[*column])
                )
            }),
            256,
            wrong.len(),
        );
        let mut wrong_depth = Vec::new();
        let mut wrong_secondary = 0;
        let mut wrong_band = Vec::new();
        let mut wrong_zoom = Vec::new();
        let (zoom_from, zoom_step, zoom_count, zoomed) = &gate.zoom;
        for column in 0..256_usize {
            let (x, z) = ((column & 15) as i32, (column >> 4) as i32);
            let (block_x, block_z) = (volume.min_x + x, volume.min_z + z);
            let (depth, secondary) = terrain.surface_depth(block_x, block_z);
            if depth != gate.depth[column] {
                wrong_depth.push(format!(
                    "column {x} {z}: {depth} here, {} in the game",
                    gate.depth[column]
                ));
            }
            wrong_secondary += usize::from(secondary.to_bits() != gate.secondary[column]);
            let band = terrain.band(block_x, 60 + x + 3 * z, block_z);
            if band != gate.band[column] {
                wrong_band.push(format!(
                    "column {x} {z}: {} here, {} in the game",
                    block_name(band),
                    block_name(gate.band[column])
                ));
            }
            for step in 0..*zoom_count {
                let y = zoom_from + step as i32 * zoom_step;
                let ours = biomes.at_block(terrain.zoom(), block_x, y, block_z);
                let theirs = zoomed[column * zoom_count + step];
                if ours != theirs {
                    wrong_zoom.push(format!(
                        "block {block_x} {y} {block_z}: {} here, {} in the game",
                        biome_name(ours),
                        biome_name(theirs)
                    ));
                }
            }
        }
        differs(
            "the surface's depth",
            wrong_depth.first().cloned().unwrap_or_default(),
            256,
            wrong_depth.len(),
        );
        differs(
            "the depth's second noise",
            String::new(),
            256,
            wrong_secondary,
        );
        differs(
            "the band of terracotta",
            wrong_band.first().cloned().unwrap_or_default(),
            256,
            wrong_band.len(),
        );
        differs(
            "the biome at a block",
            wrong_zoom.first().cloned().unwrap_or_default(),
            256 * zoom_count,
            wrong_zoom.len(),
        );
        let (asked, cold) = &gate.cold;
        let mut wrong_cold = Vec::new();
        let mut answers = cold.iter();
        for biome in asked {
            for column in 0..256_usize {
                let (block_x, block_z) = (
                    volume.min_x + (column & 15) as i32,
                    volume.min_z + (column >> 4) as i32,
                );
                for y in [64, 100, 160, 250] {
                    let theirs = *answers.next().expect("an answer for each");
                    if terrain.cold_enough_to_snow(*biome, block_x, y, block_z) != theirs {
                        wrong_cold.push(format!(
                            "{} at {block_x} {y} {block_z}: cold in the game: {theirs}",
                            biome_name(*biome)
                        ));
                    }
                }
            }
        }
        differs(
            "whether it is cold enough to snow",
            wrong_cold.first().cloned().unwrap_or_default(),
            cold.len(),
            wrong_cold.len(),
        );
        if let Some((size_y, hashes)) = &gate.veins {
            let expected = Volume {
                size_y: *size_y,
                ..volume
            };
            let densities = terrain.material_densities(&expected);
            let hash = |values: &[f32]| {
                fnv(values
                    .iter()
                    .flat_map(|value| value.to_bits().to_le_bytes()))
            };
            let mut wrong = Vec::new();
            for (place, (function, in_volume)) in [
                ("copper_density", true),
                ("richness", true),
                ("iron_density", true),
                ("gap", false),
            ]
            .iter()
            .enumerate()
            {
                let (_, sampled, alone) = densities
                    .iter()
                    .find(|(name, _, _)| name.ends_with(function))
                    .expect("the four functions of the ore veins");
                let ours = hash(if *in_volume { sampled } else { alone });
                if ours != hashes[place] {
                    wrong.push((*function).to_owned());
                }
            }
            differs(
                "the ore veins' density functions (each a hash)",
                wrong.join(", "),
                4,
                wrong.len(),
            );
        }
    }
    (counts, findings)
}

#[test]
#[ignore = "needs what tests/java/GameSurface.java wrote, which is not in the repository"]
fn chunks_have_the_surface_the_games_own_classes_give_them() {
    let Ok(directory) = std::env::var("CLUSTINE_GAME_SURFACE") else {
        println!("skipped: CLUSTINE_GAME_SURFACE names no directory of the program's output");
        return;
    };
    let mut paths: Vec<_> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("reading {directory}: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "{directory} holds no output");

    let mut failed = false;
    let mut total = (0, 0, 0, 0);
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let file = read(path);
        let (counts, findings) = judge(&file, &name);
        println!("{name}: seed {} {:?}", file.seed, file.dimension);
        println!(
            "  chunks {}, blocks {}, equal {}, different {} in {} chunks; the step changed \
             {} blocks; fills that are not the game's: {}",
            counts.chunks,
            counts.blocks,
            counts.blocks - counts.blocks_different,
            counts.blocks_different,
            counts.chunks_with_a_different_block,
            counts.blocks_changed,
            counts.fills_different
        );
        println!(
            "  marks {}, chunks whose marks differ {}",
            counts.marks, counts.marks_different
        );
        println!(
            "  gate: {} chunks, {} values, {} different; worlds whose bands differ: {}",
            counts.gate_chunks,
            counts.gate_values,
            counts.gate_values_different,
            counts.bands_different
        );
        println!(
            "  with this crate's own biomes: {} of {} cells are another biome ({} of them \
             no exact tie), in {} chunks, the first of them {:?}; that changes {} blocks in \
             {} chunks",
            counts.cells_with_another_biome,
            counts.cells,
            counts.cells_that_are_no_tie,
            counts.chunks_with_another_biome,
            counts.first_with_another_biome,
            counts.blocks_another_biome_changes,
            counts.chunks_another_biome_changes
        );
        let sorted = |counted: &BTreeMap<&'static str, usize>| -> String {
            let mut kinds: Vec<(&str, usize)> = counted
                .iter()
                .map(|(name, count)| (*name, *count))
                .collect();
            kinds.sort_by_key(|(name, count)| (std::cmp::Reverse(*count), *name));
            kinds
                .iter()
                .map(|(name, count)| format!("{count} {}", name.trim_start_matches("minecraft:")))
                .collect::<Vec<String>>()
                .join(", ")
        };
        println!(
            "  where: up to {} chunks from the origin; by the biome at the top of the \
             middle column: {}",
            counts.farthest,
            sorted(&counts.biomes)
        );
        println!("  chunks that hold: {}", sorted(&counts.with));
        println!("  blocks: {}", sorted(&counts.kinds));
        for finding in &findings {
            println!("  DIFFERENT {finding}");
        }
        total.0 += counts.chunks;
        total.1 += counts.blocks;
        total.2 += counts.blocks_different;
        total.3 += counts.marks;
        failed |= counts.blocks_different != 0
            || counts.fills_different != 0
            || counts.marks_different != 0
            || counts.gate_values_different != 0
            || counts.bands_different != 0
            || counts.cells_that_are_no_tie != 0;
    }
    println!(
        "all: chunks {}, blocks {}, equal {}, different {}, marks {}",
        total.0,
        total.1,
        total.1 - total.2,
        total.2,
        total.3
    );
    assert!(!failed, "the surface differs from the game's");
}

/// A generator of numbers for the lists: the same sequence on every machine.
struct Sequence(u64);

impl Sequence {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number from `-reach` to `reach`.
    fn within(&mut self, reach: i32) -> i32 {
        (self.next() % (2 * reach as u64 + 1)) as i32 - reach
    }

    fn chunk(&mut self, middle: (i32, i32), reach: i32) -> (i32, i32) {
        (middle.0 + self.within(reach), middle.1 + self.within(reach))
    }
}

/// The world's edge is at the block 29,999,984: the last chunk inside is this one.
const EDGE: i32 = 1_874_998;

/// Far places, as chunks: a million and ten million blocks out, the world's edge and
/// its corners.
const FAR: [(i32, i32); 11] = [
    (62_500, 0),
    (-62_500, 62_500),
    (0, -62_500),
    (625_000, 0),
    (-625_000, -625_000),
    (0, 625_000),
    (EDGE - 8, 0),
    (-EDGE + 8, 3),
    (5, EDGE - 8),
    (EDGE - 8, -EDGE + 8),
    (-EDGE + 8, EDGE - 8),
];

/// The biomes a list looks for, each with the height its cell is asked at: every
/// biome the dimension's rule names, and those that have a surface of their own
/// only by their ground.
fn kinds(dimension: Dimension) -> &'static [(&'static str, i32)] {
    match dimension {
        Dimension::Overworld => &[
            ("minecraft:badlands", 20),
            ("minecraft:eroded_badlands", 20),
            ("minecraft:wooded_badlands", 24),
            ("minecraft:frozen_ocean", 16),
            ("minecraft:deep_frozen_ocean", 16),
            ("minecraft:desert", 16),
            ("minecraft:beach", 16),
            ("minecraft:snowy_beach", 16),
            ("minecraft:stony_shore", 16),
            ("minecraft:stony_peaks", 30),
            ("minecraft:jagged_peaks", 30),
            ("minecraft:frozen_peaks", 30),
            ("minecraft:snowy_slopes", 28),
            ("minecraft:grove", 26),
            ("minecraft:swamp", 16),
            ("minecraft:mangrove_swamp", 16),
            ("minecraft:windswept_hills", 20),
            ("minecraft:windswept_gravelly_hills", 20),
            ("minecraft:windswept_savanna", 20),
            ("minecraft:old_growth_pine_taiga", 18),
            ("minecraft:old_growth_spruce_taiga", 18),
            ("minecraft:ice_spikes", 18),
            ("minecraft:mushroom_fields", 16),
            ("minecraft:dappled_forest", 18),
            ("minecraft:warm_ocean", 16),
            ("minecraft:lukewarm_ocean", 16),
            ("minecraft:deep_lukewarm_ocean", 16),
            ("minecraft:ocean", 16),
            ("minecraft:deep_ocean", 16),
            ("minecraft:cold_ocean", 16),
            ("minecraft:deep_cold_ocean", 16),
            ("minecraft:river", 16),
            ("minecraft:frozen_river", 16),
            ("minecraft:plains", 17),
            ("minecraft:snowy_plains", 17),
            ("minecraft:meadow", 26),
            ("minecraft:deep_dark", -13),
            ("minecraft:dripstone_caves", 0),
            ("minecraft:lush_caves", 0),
            ("minecraft:sulfur_caves", 0),
        ],
        Dimension::Nether => &[
            ("minecraft:basalt_deltas", 16),
            ("minecraft:nether_wastes", 16),
            ("minecraft:soul_sand_valley", 16),
            ("minecraft:crimson_forest", 16),
            ("minecraft:warped_forest", 16),
        ],
        Dimension::End => &[
            ("minecraft:end_highlands", 16),
            ("minecraft:end_midlands", 16),
            ("minecraft:small_end_islands", 16),
            ("minecraft:end_barrens", 16),
        ],
    }
}

/// The chunks of a list: `count` of them, the same for a seed and dimension every
/// time. The head of the list, which the gate takes, has a few of the first kinds.
fn list(seed: i64, dimension: Dimension, count: usize) -> Vec<(i32, i32)> {
    let biomes = BiomeSource::new(seed, dimension);
    let registry = synced_registry("minecraft:worldgen/biome").unwrap();
    let mut sequence = Sequence(seed as u64 ^ dimension as u64 ^ 0x5u64);
    let mut chunks: Vec<(i32, i32)> = Vec::new();
    let add = |chunks: &mut Vec<(i32, i32)>, chunk: (i32, i32)| {
        if !chunks.contains(&chunk) && chunks.len() < count {
            chunks.push(chunk);
        }
    };

    add(&mut chunks, (0, 0));
    add(&mut chunks, (-1, -1));
    add(&mut chunks, FAR[0]);
    add(&mut chunks, FAR[6]);

    // Kinds of ground looked for by their biome, nearer first: a rare one (the
    // mushroom fields) is looked for farther out.
    let kinds = kinds(dimension);
    let wanted: Vec<(Biome, i32)> = kinds
        .iter()
        .map(|(name, y)| (Biome(registry.id_of(name).expect("a biome") as u16), *y))
        .collect();
    let each = if count >= 1000 {
        (count * 2 / 3 / kinds.len()).min(40)
    } else {
        (count / 2 / kinds.len()).max(2)
    };
    let mut found = vec![Vec::new(); kinds.len()];
    for attempt in 0..1_500_000 {
        if found
            .iter()
            .all(|of_kind: &Vec<(i32, i32)>| of_kind.len() >= each)
        {
            break;
        }
        let reach = if attempt < 300_000 { 4000 } else { 40_000 };
        let chunk = sequence.chunk((0, 0), reach);
        for (place, (kind, y)) in wanted.iter().enumerate() {
            if found[place].len() < each
                && biomes.biome(chunk.0 * 4 + 2, *y, chunk.1 * 4 + 2) == *kind
            {
                found[place].push(chunk);
            }
        }
    }
    // One of each kind in turn, so that the gate has many kinds.
    for round in 0..each {
        for of_kind in &found {
            if let Some(chunk) = of_kind.get(round) {
                add(&mut chunks, *chunk);
            }
        }
        if round == 0 && dimension == Dimension::End {
            // The main island, which no biome tells from its surroundings.
            for chunk in [(3, -2), (-6, 4), (20, 20), (-40, 0), (0, 63), (64, 1)] {
                add(&mut chunks, chunk);
            }
        }
    }

    // Far out: each far place and chunks around it, also across the edge.
    for far in FAR {
        add(&mut chunks, far);
        for _ in 0..count / 80 {
            let chunk = sequence.chunk(far, 8);
            add(
                &mut chunks,
                (chunk.0.clamp(-EDGE, EDGE), chunk.1.clamp(-EDGE, EDGE)),
            );
        }
    }
    // Squares of four by four, where a neighbour's biomes are a chunk's ring.
    for _ in 0..count / 100 {
        let corner = sequence.chunk((0, 0), 3000);
        for x in 0..4 {
            for z in 0..4 {
                add(&mut chunks, (corner.0 + x, corner.1 + z));
            }
        }
    }
    if dimension == Dimension::End {
        // The main island and the ring of nothing around it.
        for _ in 0..count / 5 {
            add(&mut chunks, sequence.chunk((0, 0), 70));
        }
    }
    // And anywhere within 48,000 blocks.
    while chunks.len() < count {
        add(&mut chunks, sequence.chunk((0, 0), 3000));
    }
    chunks
}

#[test]
#[ignore = "writes the lists of chunks for tests/java/GameSurface.java"]
fn lists_of_chunks_for_the_game_to_make() {
    let Ok(directory) = std::env::var("CLUSTINE_GAME_SURFACE_LISTS") else {
        println!("skipped: CLUSTINE_GAME_SURFACE_LISTS names no directory to write the lists to");
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();
    let registry = synced_registry("minecraft:worldgen/biome").unwrap();
    for (place, seed) in SEEDS.iter().enumerate() {
        for (dimension, name, of_the_first_seed, of_the_others) in [
            (Dimension::Overworld, "overworld", 2400, 300),
            (Dimension::Nether, "nether", 1000, 150),
            (Dimension::End, "end", 1000, 150),
        ] {
            let count = if place == 0 {
                of_the_first_seed
            } else {
                of_the_others
            };
            let chunks = list(*seed, dimension, count);
            assert_eq!(chunks.len(), count);
            let text: String = chunks.iter().map(|(x, z)| format!("{x} {z}\n")).collect();
            let path = Path::new(&directory).join(format!("{name}_{seed}.txt"));
            std::fs::write(&path, text).unwrap();
            // Which of the kinds looked for the list has, by the biome at a chunk's
            // middle.
            let source = BiomeSource::new(*seed, dimension);
            let missing: Vec<&str> = kinds(dimension)
                .iter()
                .filter(|(kind, y)| {
                    let kind = Biome(registry.id_of(kind).unwrap() as u16);
                    !chunks
                        .iter()
                        .any(|(x, z)| source.biome(x * 4 + 2, *y, z * 4 + 2) == kind)
                })
                .map(|(kind, _)| *kind)
                .collect();
            println!(
                "{}: {count} chunks, the first {GATE} for the gate; kinds not found: {missing:?}",
                path.display()
            );
        }
    }
}

#[test]
fn the_lists_are_the_same_every_time() {
    let first = list(13579, Dimension::Nether, 40);
    assert_eq!(first, list(13579, Dimension::Nether, 40));
    assert_eq!(first.len(), 40);
    assert_eq!(first[0], (0, 0));
}
