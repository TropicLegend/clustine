//! The fill against the game's own classes: what `tests/java/GameFill.java` wrote, block
//! by block, and the density bit by bit.
//!
//! That program has the generator of the pinned server jar do its "fill from noise"
//! step for a list of chunks and nothing after it, with no server (its head says how).
//! Nothing it writes is in the repository; this test takes a directory from
//! `CLUSTINE_GAME_FILL` and reads every `.txt` file in it, each of one seed and one
//! dimension.
//!
//! ```text
//! # The lists of chunks, which this file makes (the same every time):
//! CLUSTINE_GAME_FILL_LISTS=<lists> \
//!     cargo test --release -p clustine-terrain --test game_fill -- --ignored --nocapture lists
//!
//! # With <classes> the jars `cargo datagen` unpacked (target/datagen/26.3/extract/classpath,
//! # every .jar under it); for each list `<dimension>_<seed>.txt`, with the number of
//! # chunks at the head of the list whose density is written out in full (32 here):
//! javac -proc:none -cp <classes> -d <out> crates/clustine-terrain/tests/java/GameFill.java
//! java -cp <out>:<classes> GameFill <seed> <dimension> <lists>/<dimension>_<seed>.txt 32 \
//!     <directory>/<dimension>_<seed>.txt
//!
//! CLUSTINE_GAME_FILL=<directory> \
//!     cargo test --release -p clustine-terrain --test game_fill -- --ignored --nocapture game
//! ```
//!
//! **The judge** ([`chunks_are_filled_as_the_games_own_classes_fill_them`]): for every
//! chunk, every block equal; the positions marked for post-processing equal; the
//! hash of the final density's bits equal; the height above which the aquifer stops
//! asking equal; and two columns of each chunk made alone (`getBaseColumn`) equal,
//! with the world surface the game's `getBaseHeight` gives.
//!
//! **The gate**, for the chunks at the head of each list, so that a difference in the
//! blocks can be traced to the density or to the aquifer: the density of every block
//! against the game's, bit for bit; and, for each function the final density reaches
//! through `interpolated`, the input at the corners of the cells against the game's
//! volume of corners, bit for bit. The program also counts the corners at which the
//! game's volume differs from the game's own value at the corner alone, which is
//! printed and judges nothing: it says how often the game's two ways part
//! (`clustine_noise::NormalNoise::get_in_volume` has why).
//!
//! **What was run**, on 2026-10-13 on the owner's machine (release build), and what
//! it gave. Seed 13579: 2,400 chunks of the overworld (235,929,600 blocks, 117,021
//! marks), 1,000 of the Nether and 1,000 of the End (32,768,000 blocks each). Seeds 0
//! and -7046029254386353131: 300 chunks of the overworld and 150 of each of the other
//! two, each. The lists hold the origin, chunks a million and ten million blocks out
//! and at the world's edge, chunks looked for by their biome (forty each of deep
//! dark, two kinds of peaks, shores, beaches, oceans, rivers, plains, the two kinds
//! of caves and others), squares of five by five, and chunks anywhere within 48,000
//! blocks. **Every one of 380,108,800 blocks was equal**, and so were all 143,857
//! marks, the density's hash of all 5,600 chunks, every aquifer's ceiling, and all
//! 11,200 columns made alone. Of the gate's 288 chunks every value of the density
//! (15,728,640) and every corner (657,312) was equal, to the bit. The game's
//! volume differed from the game's corner alone at one corner of those.
//!
//! Before the noises were multiplied as the game multiplies them in a volume, the
//! density of four of the 3,000 chunks of the overworld had another hash, by a few
//! last bits; no block differed even then.

use std::collections::BTreeMap;
use std::path::Path;

use clustine_data::{BlockState, synced_registry};
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::fill::{Heightmap, Terrain};
use clustine_terrain::noise_chunk::{Scratch, Volume};
use clustine_world::InChunk;

/// How many chunks at the head of a list have their density written out in full.
const GATE: usize = 32;

/// The seeds: the one every fixture of the plan has, zero, and one with the top bit
/// set.
const SEEDS: [i64; 3] = [13579, 0, -7_046_029_254_386_353_131];

/// A column the game made alone.
struct Column {
    x: i32,
    z: i32,
    world_surface: i32,
    blocks: Vec<BlockState>,
}

/// What the program wrote for one chunk.
#[derive(Default)]
struct Written {
    x: i32,
    z: i32,
    density_hash: u64,
    /// In the volume's order.
    blocks: Vec<BlockState>,
    marks: Vec<InChunk>,
    ceiling: Option<i32>,
    columns: Vec<Column>,
    /// The bits of the density, for a chunk of the gate.
    bits: Option<Vec<u32>>,
    /// For a chunk of the gate, by the program's number of the interpolated
    /// function: how many corners differed from the corner alone, and the bits.
    corners: Vec<(usize, Vec<u32>)>,
}

/// What the program wrote for one seed and dimension.
struct File {
    seed: i64,
    dimension: Dimension,
    min_y: i32,
    height: i32,
    /// The cell sizes of the interpolated functions, across and up, by the
    /// program's number.
    interpolated: Vec<(i32, i32)>,
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

fn hex(text: &str) -> Vec<u32> {
    text.as_bytes()
        .chunks(8)
        .map(|digits| u32::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap())
        .collect()
}

fn read(path: &Path) -> File {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let mut file = File {
        seed: 0,
        dimension: Dimension::Overworld,
        min_y: 0,
        height: 0,
        interpolated: Vec::new(),
        chunks: Vec::new(),
    };
    let mut ended = false;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        match fields[..] {
            ["game-fill", "1"] => {}
            ["seed", seed] => file.seed = seed.parse().unwrap(),
            ["dimension", name] => {
                file.dimension = match name {
                    "overworld" => Dimension::Overworld,
                    "nether" => Dimension::Nether,
                    "end" => Dimension::End,
                    other => panic!("no dimension {other}"),
                }
            }
            [
                "noise",
                min_y,
                height,
                _sea_level,
                _default_block,
                _default_fluid,
            ] => {
                file.min_y = min_y.parse().unwrap();
                file.height = height.parse().unwrap();
            }
            ["constants", way_below] => assert_eq!(way_below, "-32512"),
            ["interpolated", number, xz, y] => {
                assert_eq!(number.parse(), Ok(file.interpolated.len()));
                file.interpolated
                    .push((xz.parse().unwrap(), y.parse().unwrap()));
            }
            ["chunk", x, z] => file.chunks.push(Written {
                x: x.parse().unwrap(),
                z: z.parse().unwrap(),
                ..Written::default()
            }),
            ["end", chunks] => {
                assert_eq!(chunks.parse(), Ok(file.chunks.len()));
                ended = true;
            }
            _ => {
                let chunk = file
                    .chunks
                    .last_mut()
                    .expect("a chunk's line after a chunk");
                match fields[..] {
                    ["density", hash] => {
                        chunk.density_hash = u64::from_str_radix(hash, 16).unwrap();
                    }
                    ["blocks", ..] => chunk.blocks = runs(&fields[1..]),
                    ["marks", ..] => {
                        chunk.marks = fields[1..]
                            .iter()
                            .map(|mark| {
                                let parts: Vec<i32> =
                                    mark.split(',').map(|part| part.parse().unwrap()).collect();
                                InChunk::new(parts[0] as usize, parts[1], parts[2] as usize)
                            })
                            .collect();
                    }
                    ["ceiling", "none"] => chunk.ceiling = None,
                    ["ceiling", y] => chunk.ceiling = Some(y.parse().unwrap()),
                    ["column", x, z, world_surface, ..] => chunk.columns.push(Column {
                        x: x.parse().unwrap(),
                        z: z.parse().unwrap(),
                        world_surface: world_surface.parse().unwrap(),
                        blocks: runs(&fields[4..]),
                    }),
                    ["bits", bits] => chunk.bits = Some(hex(bits)),
                    ["corners", number, differing, bits] => {
                        assert_eq!(number.parse(), Ok(chunk.corners.len()));
                        chunk.corners.push((differing.parse().unwrap(), hex(bits)));
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

/// The hash the program takes of a density: FNV-1a over each value's bits, the
/// lowest byte first.
fn hash(density: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for value in density {
        for byte in value.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

fn biome_name(source: &BiomeSource, x: i32, y: i32, z: i32) -> &'static str {
    let biome = source.biome(x, y, z);
    synced_registry("minecraft:worldgen/biome").unwrap().entries[usize::from(biome.0)]
}

/// What was counted over one file.
#[derive(Default)]
struct Counts {
    chunks: usize,
    blocks: usize,
    blocks_different: usize,
    chunks_with_a_different_block: usize,
    densities_different: usize,
    marks: usize,
    marks_different: usize,
    ceilings_different: usize,
    columns: usize,
    columns_different: usize,
    surfaces_different: usize,
    gate_chunks: usize,
    gate_values: usize,
    gate_values_different: usize,
    corner_values: usize,
    corner_values_different: usize,
    corners_differing_from_the_corner_alone: usize,
    /// Of the blocks, by state.
    states: BTreeMap<u16, usize>,
    /// Of the chunks, by the biome at their middle at the height 64, and how many
    /// have the deep dark below.
    biomes: BTreeMap<&'static str, usize>,
    deep_dark: usize,
    /// The farthest chunk from the origin, in chunks.
    farthest: i64,
}

fn judge(file: &File, name: &str) -> (Counts, Vec<String>) {
    let terrain = Terrain::new(file.seed, file.dimension);
    let biomes = BiomeSource::new(file.seed, file.dimension);
    let settings = terrain.settings();
    assert_eq!((settings.min_y, settings.height), (file.min_y, file.height));
    let height = file.height as usize;
    let mut counts = Counts::default();
    let mut findings = Vec::new();
    let mut find = |text: String| {
        if findings.len() < 40 {
            findings.push(text);
        }
    };

    // Which of this crate's interpolated functions is which of the program's is
    // found by their corners; the numbers need not agree.
    let ours = terrain.interpolated();
    assert_eq!(
        ours.len(),
        file.interpolated.len(),
        "{name}: as many interpolated functions as the game's final density reaches"
    );

    let mut scratch = Scratch::default();
    for written in &file.chunks {
        counts.chunks += 1;
        counts.farthest = counts
            .farthest
            .max(i64::from(written.x).abs())
            .max(i64::from(written.z).abs());
        *counts
            .biomes
            .entry(biome_name(
                &biomes,
                written.x * 4 + 2,
                16,
                written.z * 4 + 2,
            ))
            .or_default() += 1;
        if file.dimension == Dimension::Overworld
            && biome_name(&biomes, written.x * 4 + 2, -13, written.z * 4 + 2)
                == "minecraft:deep_dark"
        {
            counts.deep_dark += 1;
        }

        let volume: Volume = terrain.volume(written.x, written.z);
        assert_eq!(written.blocks.len(), volume.len());
        let (filled, density) = terrain.fill_with_density(written.x, written.z, &mut scratch);

        // The density.
        if hash(&density) != written.density_hash {
            counts.densities_different += 1;
            find(format!(
                "chunk {} {}: the density's hash is {:x}, the game's {:x}",
                written.x,
                written.z,
                hash(&density),
                written.density_hash
            ));
        }
        if let Some(bits) = &written.bits {
            counts.gate_chunks += 1;
            for z in 0..16 {
                for x in 0..16 {
                    for y in 0..height {
                        let index = volume.index(x, y, z);
                        counts.gate_values += 1;
                        if density[index].to_bits() != bits[index] {
                            counts.gate_values_different += 1;
                            find(format!(
                                "density at {} {} {}: here {:e} ({:08x}), the game {:e} ({:08x})",
                                volume.min_x + x as i32,
                                volume.min_y + y as i32,
                                volume.min_z + z as i32,
                                density[index],
                                density[index].to_bits(),
                                f32::from_bits(bits[index]),
                                bits[index]
                            ));
                        }
                    }
                }
            }
            // The corners: each of the game's functions is one of ours.
            let mut taken = vec![false; ours.len()];
            for (number, (alone, theirs)) in written.corners.iter().enumerate() {
                counts.corners_differing_from_the_corner_alone += alone;
                let (cell_xz, cell_y) = file.interpolated[number];
                let sizes = [
                    (16 / cell_xz + 1) as usize,
                    (file.height / cell_y + 1) as usize,
                    (16 / cell_xz + 1) as usize,
                ];
                let mut best: Option<(usize, usize)> = None;
                for (place, (our_number, our_xz, our_y)) in ours.iter().enumerate() {
                    if taken[place] || (*our_xz, *our_y) != (cell_xz, cell_y) {
                        continue;
                    }
                    let values = scratch
                        .corners_in_the_games_order(*our_number, sizes)
                        .expect("the fill left the corners of every function it reaches");
                    assert_eq!(values.len(), theirs.len());
                    let different = values
                        .iter()
                        .zip(theirs)
                        .filter(|(ours, theirs)| ours.to_bits() != **theirs)
                        .count();
                    if best.is_none_or(|(_, least)| different < least) {
                        best = Some((place, different));
                    }
                }
                let (place, different) = best.expect("a function of the same cell sizes is left");
                taken[place] = true;
                counts.corner_values += theirs.len();
                counts.corner_values_different += different;
                if different > 0 {
                    find(format!(
                        "chunk {} {}: {different} of {} corners of the game's interpolated \
                         function {number} differ from this crate's nearest, number {}",
                        written.x,
                        written.z,
                        theirs.len(),
                        ours[place].0
                    ));
                }
            }
        }

        // The blocks.
        let mut different_here = 0;
        for z in 0..16 {
            for x in 0..16 {
                for y in 0..height {
                    let index = volume.index(x, y, z);
                    let block_y = volume.min_y + y as i32;
                    let here = filled.get(x, block_y, z);
                    let theirs = written.blocks[index];
                    counts.blocks += 1;
                    *counts.states.entry(theirs.0).or_default() += 1;
                    if here != theirs {
                        different_here += 1;
                        let game_density = written.bits.as_ref().map_or_else(
                            || "not written".to_owned(),
                            |bits| format!("{:e}", f32::from_bits(bits[index])),
                        );
                        find(format!(
                            "block at {} {} {}: here {}, the game {}; density here {:e}, the \
                             game's {game_density}",
                            volume.min_x + x as i32,
                            block_y,
                            volume.min_z + z as i32,
                            here.0,
                            theirs.0,
                            density[index],
                        ));
                    }
                }
            }
        }
        counts.blocks_different += different_here;
        counts.chunks_with_a_different_block += usize::from(different_here > 0);

        // The marks: the chunk kept them in the order they were made, here they are
        // ascending.
        let mut marks = written.marks.clone();
        marks.sort_unstable();
        counts.marks += marks.len();
        if marks != filled.marks {
            let different = marks
                .iter()
                .filter(|mark| !filled.marks.contains(mark))
                .count()
                + filled
                    .marks
                    .iter()
                    .filter(|mark| !marks.contains(mark))
                    .count();
            counts.marks_different += different;
            find(format!(
                "chunk {} {}: {} marks here, {} of the game's, {different} in one and not the other",
                written.x,
                written.z,
                filled.marks.len(),
                marks.len()
            ));
        }

        let ceiling = terrain.aquifer_ceiling(written.x, written.z);
        if ceiling != written.ceiling {
            counts.ceilings_different += 1;
            find(format!(
                "chunk {} {}: the aquifer stops asking above {ceiling:?} here, {:?} in the game",
                written.x, written.z, written.ceiling
            ));
        }

        for column in &written.columns {
            counts.columns += 1;
            let here = terrain.base_column(column.x, column.z);
            if here != column.blocks {
                counts.columns_different += 1;
                let at = here
                    .iter()
                    .zip(&column.blocks)
                    .position(|(ours, theirs)| ours != theirs);
                find(format!(
                    "the column {} {} made alone differs from the game's, first at {:?} from \
                     the bottom",
                    column.x, column.z, at
                ));
            }
            let surface = terrain.base_height(column.x, column.z, Heightmap::WorldSurfaceWg);
            if surface != column.world_surface {
                counts.surfaces_different += 1;
                find(format!(
                    "the world surface of the column {} {} is {surface} here, {} in the game",
                    column.x, column.z, column.world_surface
                ));
            }
            // The ocean floor cannot be asked of the game without a server; it is
            // at or below the world surface, and at it where no fluid lies on top.
            let floor = terrain.base_height(column.x, column.z, Heightmap::OceanFloorWg);
            assert!(floor <= surface);
        }
    }
    (counts, findings)
}

#[test]
#[ignore = "needs what tests/java/GameFill.java writes from the server jar"]
fn chunks_are_filled_as_the_games_own_classes_fill_them() {
    let Ok(directory) = std::env::var("CLUSTINE_GAME_FILL") else {
        println!("skipped: CLUSTINE_GAME_FILL names no directory with the game's fills");
        return;
    };
    let mut paths: Vec<_> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("reading {directory}: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "{directory} has no file of the program's"
    );

    let mut failed = false;
    for path in &paths {
        let file = read(path);
        let name = format!("{:?}, seed {}", file.dimension, file.seed);
        let (counts, findings) = judge(&file, &name);
        println!(
            "{name}: {} chunks, {} blocks, {} equal, {} different in {} chunks",
            counts.chunks,
            counts.blocks,
            counts.blocks - counts.blocks_different,
            counts.blocks_different,
            counts.chunks_with_a_different_block
        );
        let states: Vec<String> = counts
            .states
            .iter()
            .map(|(state, count)| format!("{count} of state {state}"))
            .collect();
        println!("  the game's blocks: {}", states.join(", "));
        println!(
            "  density: {} of {} chunks have another hash than the game's",
            counts.densities_different, counts.chunks
        );
        println!(
            "  gate: {} chunks, {} values of the density, {} different; {} corners, {} \
             different; the game's volume differs from the game's corner alone at {}",
            counts.gate_chunks,
            counts.gate_values,
            counts.gate_values_different,
            counts.corner_values,
            counts.corner_values_different,
            counts.corners_differing_from_the_corner_alone
        );
        println!(
            "  marks: {} of the game's, {} in one and not the other; aquifer ceilings: {} \
             different",
            counts.marks, counts.marks_different, counts.ceilings_different
        );
        println!(
            "  columns made alone: {}, {} different, {} with another world surface",
            counts.columns, counts.columns_different, counts.surfaces_different
        );
        let mut kinds: Vec<(&str, usize)> = counts
            .biomes
            .iter()
            .map(|(name, count)| (*name, *count))
            .collect();
        kinds.sort_by_key(|(name, count)| (std::cmp::Reverse(*count), *name));
        let kinds: Vec<String> = kinds
            .iter()
            .map(|(name, count)| format!("{count} {}", name.trim_start_matches("minecraft:")))
            .collect();
        println!(
            "  where: up to {} chunks from the origin; {} chunks over the deep dark; by the \
             biome at the height 64: {}",
            counts.farthest,
            counts.deep_dark,
            kinds.join(", ")
        );
        for finding in &findings {
            println!("  DIFFERENT {finding}");
        }
        failed |= counts.blocks_different != 0
            || counts.densities_different != 0
            || counts.gate_values_different != 0
            || counts.corner_values_different != 0
            || counts.marks_different != 0
            || counts.ceilings_different != 0
            || counts.columns_different != 0
            || counts.surfaces_different != 0;
    }
    assert!(!failed, "the fill differs from the game's");
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
/// its corners, and the first cluster of SteelMC's fixtures.
const FAR: [(i32, i32); 12] = [
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
    (-418_462, 366_791),
];

/// The chunks of a list: `count` of them, the same for a seed and dimension every
/// time. The head of the list, which the gate takes, has a few of every kind.
fn list(seed: i64, dimension: Dimension, count: usize) -> Vec<(i32, i32)> {
    let biomes = BiomeSource::new(seed, dimension);
    let mut sequence = Sequence(seed as u64 ^ dimension as u64);
    let mut chunks: Vec<(i32, i32)> = Vec::new();
    let add = |chunks: &mut Vec<(i32, i32)>, chunk: (i32, i32)| {
        if !chunks.contains(&chunk) && chunks.len() < count {
            chunks.push(chunk);
        }
    };

    // The head: the origin, each far place, and kinds of terrain looked for near
    // the origin by their biome.
    add(&mut chunks, (0, 0));
    add(&mut chunks, (-1, -1));
    for far in FAR {
        add(&mut chunks, far);
    }
    let kinds: &[(&str, i32)] = match dimension {
        Dimension::Overworld => &[
            ("minecraft:deep_dark", -13),
            ("minecraft:jagged_peaks", 16),
            ("minecraft:frozen_peaks", 16),
            ("minecraft:stony_shore", 16),
            ("minecraft:beach", 16),
            ("minecraft:deep_ocean", 16),
            ("minecraft:ocean", 16),
            ("minecraft:river", 16),
            ("minecraft:plains", 16),
            ("minecraft:dripstone_caves", 0),
            ("minecraft:lush_caves", 0),
            ("minecraft:windswept_hills", 16),
            ("minecraft:badlands", 16),
            ("minecraft:swamp", 16),
        ],
        Dimension::Nether => &[
            ("minecraft:basalt_deltas", 16),
            ("minecraft:nether_wastes", 16),
            ("minecraft:soul_sand_valley", 16),
        ],
        Dimension::End => &[
            ("minecraft:end_highlands", 16),
            ("minecraft:small_end_islands", 16),
            ("minecraft:end_barrens", 16),
        ],
    };
    let each = if count >= 1000 { 40 } else { 4 };
    let mut found = vec![Vec::new(); kinds.len()];
    for _ in 0..200_000 {
        if found
            .iter()
            .all(|of_kind: &Vec<(i32, i32)>| of_kind.len() >= each)
        {
            break;
        }
        let chunk = sequence.chunk((0, 0), 4000);
        for (place, (kind, y)) in kinds.iter().enumerate() {
            if found[place].len() < each
                && biome_name(&biomes, chunk.0 * 4 + 2, *y, chunk.1 * 4 + 2) == *kind
            {
                found[place].push(chunk);
            }
        }
    }
    // One of each kind in turn, so that the gate has every kind.
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

    // Far out: around each far place, also across the edge where there is one.
    for far in FAR {
        for _ in 0..count / 60 {
            let chunk = sequence.chunk(far, 8);
            add(
                &mut chunks,
                (chunk.0.clamp(-EDGE, EDGE), chunk.1.clamp(-EDGE, EDGE)),
            );
        }
    }
    // Squares of five by five, where neighbours share corners and aquifers.
    for _ in 0..count / 125 {
        let corner = sequence.chunk((0, 0), 3000);
        for x in 0..5 {
            for z in 0..5 {
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
#[ignore = "writes the lists of chunks for tests/java/GameFill.java"]
fn lists_of_chunks_for_the_game_to_fill() {
    let Ok(directory) = std::env::var("CLUSTINE_GAME_FILL_LISTS") else {
        println!("skipped: CLUSTINE_GAME_FILL_LISTS names no directory to write the lists to");
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();
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
            println!(
                "{}: {count} chunks, the first {GATE} for the gate",
                path.display()
            );
        }
    }
}
