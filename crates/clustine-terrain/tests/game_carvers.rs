//! The carvers against the game's own classes: what `tests/java/GameCarvers.java`
//! wrote, block by block.
//!
//! That program has the generator of the pinned server jar do its "fill from noise",
//! its "build surface" and its "generate carvers" step for a list of chunks and
//! nothing after them, with no server (its head says how, and what it builds by
//! hand). Nothing it writes is in the repository; this test takes a directory from
//! `CLUSTINE_GAME_CARVERS` and reads every `.txt` file in it, each of one seed and one
//! dimension.
//!
//! ```text
//! # The lists of chunks, which this file makes (the same every time; it runs this
//! # crate's carvers over some ten thousand chunks to find those worth judging):
//! CLUSTINE_GAME_CARVERS_LISTS=<lists> \
//!     cargo test --release -p clustine-terrain --test game_carvers -- --ignored --nocapture lists_of
//!
//! # With <classes> the jars `cargo datagen` unpacked (target/datagen/26.3/extract/classpath,
//! # every .jar under it); for each list `<dimension>_<seed>.txt`, from a directory of
//! # its own (the game writes `logs/latest.log` where it is run):
//! javac -proc:none -cp <classes> -d <out> crates/clustine-terrain/tests/java/GameCarvers.java
//! java -cp <out>:<classes> GameCarvers <seed> <dimension> <lists>/<dimension>_<seed>.txt \
//!     <directory>/<dimension>_<seed>.txt
//!
//! CLUSTINE_GAME_CARVERS=<directory> \
//!     cargo test --release -p clustine-terrain --test game_carvers -- --ignored --nocapture carved_as
//! ```
//!
//! **The judge** ([`chunks_are_carved_as_the_games_own_classes_carve_them`]): for
//! every chunk, the carving mask equal bit for bit, every block after the carvers
//! equal, and the positions marked for post-processing equal, each as often as the
//! game marked it (it marks a fluid of the fill again when a carver reaches it). The
//! chunk is made **with the biomes the program read**, because a biome that is an
//! exact tie is not always the game's here (`clustine_terrain::biome`): the cells of
//! the chunk and a ring around it as its chunks stored them, for the surface; the
//! biome each of the 289 source chunks took its carvers from; and the cells the
//! game's top material asked its biome source for, which are put over the stored
//! ones. Beside that the test makes the chunk with this crate's own biomes
//! (`Terrain::carved`) and counts the chunks and blocks that differ from the game's:
//! they are printed and judge nothing.
//!
//! The fill and the surface are held to the game's by a hash, as a gate: a
//! difference there is not the carvers'. For a block that differs the test prints
//! what it was after the surface, whether each side's mask names it, and which
//! carvers from which source chunks reached it on this side.
//!
//! **The top material by itself.** The carvers ask the game's `topMaterial` at few
//! blocks, nearly all of them dirt under grass on level ground, where it answers
//! grass whatever the slope is. So the program also asks it between the surface and
//! the carvers at three blocks of every column, each with a fluid above and without
//! (its head says where), and the test holds `Terrain::top_material` to every answer.
//! Where the game's biome source gave that one answer another biome than the chunks
//! stored, which is a tie its thread decided otherwise that time, the answer is
//! made with that biome.
//!
//! **What was run**, on 2026-10-13 on the owner's machine (release build; the game
//! took two minutes and a quarter for the longest list, the judge a minute and a
//! half), and what it gave. Seed 13579: 2,400 chunks of the overworld (235,929,600
//! blocks, of which the step changed 2,330,833; 179,209 marks, 56,457 of them the
//! step's, 4,795 of them of a position marked before), 1,000 of the Nether
//! (32,768,000 blocks, 1,849,511 changed, 426 marks) and 100 of the End (which the
//! step leaves as they are: no biome of it has a carver). Seeds 0 and
//! -7046029254386353131: 300 chunks of the overworld, 300 of the Nether and 40 of the
//! End, each. **Every one of 353,239,040 blocks of 4,780 chunks was equal**, every
//! mask bit for bit (9,630,035 blocks named), all 218,260 marks, every surface before
//! it, and all 7,342,080 answers of the top material by itself.
//!
//! The lists are made by running this crate's carvers over chunks within 48,000
//! blocks and keeping those of each kind: reached by a canyon (428 of the overworld's
//! 2,400), a canyon under an ocean, a canyon and both kinds of caves, many caves,
//! deep caves, caves under an ocean, under a river, a swamp or a beach, under
//! mushroom fields, under a desert or badlands, under snow or peaks, and chunks a
//! carver only grazes; in the Nether many caves and caves in each kind of biome.
//! To them come the origin, chunks a million and ten million blocks out and at the
//! world's edge, where the source chunks lie beyond it, squares of four by four,
//! and chunks anywhere. In the overworld of seed 13579 the carvers carved away
//! stone and deepslate, 24,448 blocks of dirt, 4,615 of grass and 241 of mycelium,
//! sand, sandstone and gravel of sea floors and deserts, the terracotta of the
//! badlands, packed ice, snow and the ores of the veins; 1,591,508 blocks became
//! air, 671,107 water (carved sea floors and aquifers) and 66,127 lava. The masks
//! named 5,929 blocks of bedrock there, which the game left (so the tag bound by
//! hand is at work), and 114,997 more where the aquifer says ground. The game asked
//! the top material 10,028 times in 206 chunks, and 1,966 blocks of dirt became grass
//! and 122 mycelium. In the Nether 568,822 blocks became lava and 1,280,689 air.
//!
//! With this crate's own biomes in place of the program's, 293 source chunks of
//! 1,381,420 have another biome, every one with the same carvers; the whole of fill,
//! surface and carving then differs from the game's in 955 blocks of 16 chunks,
//! which are the surface's ties (`tests/game_surface.rs`).
//!
//! **What the judge does not see.** In all 14,122 carvers that reached their chunk
//! none started eight chunks away: a carver walks at most seven chunks, and the
//! game looks eight away all the same, as this crate does. And no answer of the top
//! material depends on whether a fluid is above: the game's own answers are the same
//! with and without in every one of 3,671,040 pairs, so with the game's rules that
//! argument changes nothing, and passing it wrongly would not show.
//!
//! **That the judge can fail** was tried on the 600 chunks of seed 0 (and where
//! that showed nothing, on the 2,400 of the overworld of seed 13579), one change at
//! a time: a steep tunnel keeping 0.9 of its pitch in place of 0.92 (41,773 blocks
//! in 184 chunks of the overworld, 218 masks); a canyon's width taken from the
//! height itself in place of the one below (2,644 blocks in 44 chunks); every
//! carver of a biome seeded as the first (281,653 blocks in 270 chunks); a tunnel
//! splitting from a thickness of 0.9 (34,505 blocks); a room around the cave's
//! start in place of one block east of it (2,048 blocks in 41 chunks); bedrock
//! carved (438 blocks in 24 chunks of the overworld and 5,476 of the Nether, the
//! masks equal); a carved fluid never marked (the marks of 125 chunks); grass
//! counted only for the block it is and not for the rest of its run (one block of
//! one chunk in 2,400: dirt for grass); the top material given no slope (1,670
//! answers of the top material by itself, snow for stone on the snowy slopes; no
//! block of a carved chunk). Two changes showed nothing, and cannot: a cave's floor
//! left out below a height in place of at and below it, which differs only where a
//! block's middle is exactly at the floor; and the top material never told of a
//! fluid above (see above).

use std::collections::BTreeMap;
use std::path::Path;

use clustine_data::{BLOCKS, BlockState, synced_registry};
use clustine_terrain::biome::{BiomeSource, Dimension};
use clustine_terrain::carver::{Mask, REACH, SOURCES};
use clustine_terrain::fill::Terrain;
use clustine_terrain::noise_chunk::Scratch;
use clustine_terrain::surface::Biomes;
use clustine_world::{Biome, InChunk};

/// The seeds: the one every fixture of the plan has, zero, and one with the top bit
/// set.
const SEEDS: [i64; 3] = [13579, 0, -7_046_029_254_386_353_131];

/// How many answers of the top material by itself the program writes for a column:
/// three blocks, each without a fluid above and with one.
const TOP_OF_A_COLUMN: usize = 6;

/// How many source chunks there are along one side.
const ACROSS: i32 = 2 * REACH + 1;

/// What the program wrote for one chunk.
struct Written {
    x: i32,
    z: i32,
    /// The 36 columns of cells, by our numbers.
    biomes: Vec<Vec<Biome>>,
    surfaced_hash: u64,
    /// The top material by itself: for each column three blocks, each without a
    /// fluid above and with one. `None` where there is no such block; else what the
    /// rule gave, which can be nothing.
    top: Vec<Option<Option<BlockState>>>,
    /// For an answer by its place: a cell it asked the biome source for whose answer
    /// was not what the chunks stored.
    top_asked: Vec<(usize, (i32, i32, i32, Biome))>,
    /// The biome of each source chunk, x counting most.
    sources: Vec<Biome>,
    /// The other cells the step asked the biome source for.
    asked: Vec<(i32, i32, i32, Biome)>,
    /// After the carvers, in the volume's order.
    blocks: Vec<BlockState>,
    /// Sorted; a position can stand more than once.
    marks: Vec<InChunk>,
    mask_heights: (i32, i32),
    /// The runs of set bits: the first index and the last.
    mask: Vec<(usize, usize)>,
}

/// What the program wrote for one seed and dimension.
struct File {
    seed: i64,
    dimension: Dimension,
    min_y: i32,
    height: i32,
    /// The lowest height the carvers read and how many blocks high.
    context: (i32, i32),
    uncarvable: Vec<BlockState>,
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

fn read(path: &Path) -> File {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let registry = synced_registry("minecraft:worldgen/biome").unwrap();
    let mut file = File {
        seed: 0,
        dimension: Dimension::Overworld,
        min_y: 0,
        height: 0,
        context: (0, 0),
        uncarvable: Vec::new(),
        chunks: Vec::new(),
    };
    // The program's numbers of the biomes, as ours.
    let mut biome_of: Vec<Biome> = Vec::new();
    let mut ended = false;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        match fields[..] {
            ["game-carvers", "1"] => {}
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
            ["context", min_y, depth] => {
                file.context = (min_y.parse().unwrap(), depth.parse().unwrap());
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
                surfaced_hash: 0,
                top: Vec::new(),
                top_asked: Vec::new(),
                sources: Vec::new(),
                asked: Vec::new(),
                blocks: Vec::new(),
                marks: Vec::new(),
                mask_heights: (0, 0),
                mask: Vec::new(),
            }),
            _ => {
                if fields[0] == "uncarvable" {
                    file.uncarvable = fields[1..]
                        .iter()
                        .map(|state| BlockState(state.parse().unwrap()))
                        .collect();
                    continue;
                }
                let chunk = file
                    .chunks
                    .last_mut()
                    .expect("a chunk's line after a chunk");
                let biome = |number: &str| biome_of[number.parse::<usize>().unwrap()];
                let cells = |fields: &[&str]| -> Vec<(i32, i32, i32, Biome)> {
                    fields
                        .iter()
                        .map(|asked| {
                            let parts: Vec<&str> = asked.split(',').collect();
                            (
                                parts[0].parse().unwrap(),
                                parts[1].parse().unwrap(),
                                parts[2].parse().unwrap(),
                                biome(parts[3]),
                            )
                        })
                        .collect()
                };
                match fields[0] {
                    "biomes" => {
                        chunk.biomes = fields[1..]
                            .iter()
                            .map(|column| {
                                let mut cells = Vec::new();
                                for run in column.split(',') {
                                    let (of, count) = run.split_once('*').unwrap();
                                    cells.extend(std::iter::repeat_n(
                                        biome(of),
                                        count.parse().unwrap(),
                                    ));
                                }
                                cells
                            })
                            .collect();
                        assert_eq!(chunk.biomes.len(), 36);
                    }
                    "surfaced" => chunk.surfaced_hash = u64::from_str_radix(fields[1], 16).unwrap(),
                    "sources" => {
                        chunk.sources = fields[1..].iter().map(|number| biome(number)).collect();
                        assert_eq!(chunk.sources.len(), SOURCES);
                    }
                    "asked" => chunk.asked = cells(&fields[1..]),
                    "topasked" => {
                        chunk.top_asked = fields[1..]
                            .iter()
                            .map(|asked| {
                                let (answer, cell) = asked.split_once(':').unwrap();
                                (answer.parse().unwrap(), cells(&[cell])[0])
                            })
                            .collect();
                    }
                    "top" => {
                        chunk.top = fields[1..]
                            .iter()
                            .map(|given| match *given {
                                "x" => None,
                                "-" => Some(None),
                                state => Some(Some(BlockState(state.parse().unwrap()))),
                            })
                            .collect();
                        assert_eq!(chunk.top.len(), 256 * TOP_OF_A_COLUMN);
                    }
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
                    "mask" => {
                        chunk.mask_heights =
                            (fields[1].parse().unwrap(), fields[2].parse().unwrap());
                        chunk.mask = fields[3..]
                            .iter()
                            .map(|run| {
                                let (first, count) = run.split_once('+').unwrap();
                                let first: usize = first.parse().unwrap();
                                (first, first + count.parse::<usize>().unwrap() - 1)
                            })
                            .collect();
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
    state
        .block_id()
        .map_or("no block", |id| BLOCKS[id].name)
        .trim_start_matches("minecraft:")
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
    surfaces_different: usize,
    marks: usize,
    /// Marks the game has more than once.
    marks_again: usize,
    chunks_whose_marks_differ: usize,
    masks_different: usize,
    /// Blocks the masks name, and chunks whose mask names any.
    reached: usize,
    chunks_reached: usize,
    /// Blocks the step changed, by the game's output, and what they became.
    changed: usize,
    became: BTreeMap<&'static str, usize>,
    /// What was carved away, by block.
    was: BTreeMap<&'static str, usize>,
    /// Blocks a mask names that are of the tag the carvers leave.
    uncarvable_reached: usize,
    /// Blocks a mask names where the aquifer says ground, or what was there.
    reached_and_left: usize,
    /// How often the top material was asked, in how many chunks, and what it gave
    /// where that was not the dirt that was there.
    top_material_asked: usize,
    chunks_with_top_material: usize,
    top_material_gave: BTreeMap<&'static str, usize>,
    /// The top material by itself: answers, those that differ, what was answered,
    /// answers that a fluid above changes, and those on a slope the rule calls
    /// steep.
    top_answers: usize,
    top_answers_different: usize,
    top_answered: BTreeMap<&'static str, usize>,
    top_answers_a_fluid_changes: usize,
    top_cells_not_as_stored: usize,
    /// Fluids the step put, below the sea level and at or above it, and how many it
    /// marked.
    fluids_put: usize,
    marks_new: usize,
    /// Chunks that a carver of each name reaches, and carvers that start.
    reaching: BTreeMap<&'static str, usize>,
    started: usize,
    started_reaching: usize,
    /// Of those that reach, the ones that start eight chunks away.
    reaching_from_the_rim: usize,
    /// Of the chunks, by the biome at the highest block of their middle column.
    biomes: BTreeMap<&'static str, usize>,
    /// Cells the top material asked that are not what the chunks stored, and those
    /// outside the chunk's ring of cells or the dimension's heights.
    asked_cells: usize,
    asked_cells_not_as_stored: usize,
    asked_cells_outside: usize,
    /// With this crate's own biomes in place of the program's.
    sources_with_another_biome: usize,
    sources_with_other_carvers: usize,
    chunks_own_biomes_change: usize,
    blocks_own_biomes_change: usize,
    farthest: i64,
}

fn judge(file: &File) -> (Counts, Vec<String>) {
    let terrain = Terrain::new(file.seed, file.dimension);
    let source = BiomeSource::new(file.seed, file.dimension);
    let settings = terrain.settings();
    assert_eq!((settings.min_y, settings.height), (file.min_y, file.height));
    let heights = terrain.carver_heights();
    assert_eq!((heights.min_y, heights.depth), file.context);
    let height = file.height as usize;
    let mut counts = Counts::default();
    let mut findings = Vec::new();
    let mut find = |text: String| {
        if findings.len() < 30 {
            findings.push(text);
        }
    };

    let mut scratch = Scratch::default();
    for written in &file.chunks {
        counts.chunks += 1;
        counts.farthest = counts
            .farthest
            .max(i64::from(written.x).abs())
            .max(i64::from(written.z).abs());
        let volume = terrain.volume(written.x, written.z);
        assert_eq!(written.blocks.len(), volume.len());

        // The biomes the game read: the stored cells, and over them the cells its
        // top material asked the source for.
        let mut columns = written.biomes.iter();
        let stored = Biomes::from_columns(file.dimension, written.x, written.z, |_, _| {
            columns.next().expect("36 columns").clone()
        });
        let in_ring = |x: &i32, z: &i32| {
            (written.x * 4 - 1..=written.x * 4 + 4).contains(x)
                && (written.z * 4 - 1..=written.z * 4 + 4).contains(z)
        };
        let mut biomes = stored.clone();
        for (x, y, z, biome) in &written.asked {
            counts.asked_cells += 1;
            if in_ring(x, z) && stored.cell(*x, *y, *z) != *biome {
                counts.asked_cells_not_as_stored += 1;
            }
            if !biomes.set_cell(*x, *y, *z, *biome) {
                counts.asked_cells_outside += 1;
                find(format!(
                    "chunk {} {}: the top material asked the cell {x} {y} {z}, which is not of \
                     the chunk's biomes",
                    written.x, written.z
                ));
            }
        }
        let biome_of_source = |x: i32, z: i32| {
            written.sources[((x - written.x + REACH) * ACROSS + (z - written.z + REACH)) as usize]
        };

        // The fill and the surface, by their hash.
        let (mut made, _) = terrain.fill_with_density(written.x, written.z, &mut scratch);
        terrain.build_surface(&mut made, written.x, written.z, &biomes, &mut scratch);
        let surfaced = in_the_volumes_order(&made.blocks, height);
        if fnv(surfaced.iter().flat_map(|state| state.0.to_le_bytes())) != written.surfaced_hash {
            counts.surfaces_different += 1;
            find(format!(
                "chunk {} {}: the surface is not the game's",
                written.x, written.z
            ));
        }
        let marks_before = made.marks.len();

        // The top material by itself, on the chunk after its surface, with the
        // biomes the chunks stored but where the game's biome source answered
        // otherwise for that one answer: a tie.
        counts.top_cells_not_as_stored += written.top_asked.len();
        // Where each answer was asked: the block, and whether a fluid is above.
        let mut places: Vec<Option<(i32, i32, i32, bool)>> = Vec::with_capacity(written.top.len());
        for in_z in 0..16 {
            for in_x in 0..16 {
                let ground = |from: i32| {
                    (file.min_y..=from).rev().find(|y| {
                        let state = made.get(in_x, *y, in_z);
                        !state.is_air() && !state.has_fluid()
                    })
                };
                let top = file.min_y + file.height - 1;
                let highest = ground(top);
                let asked = [
                    highest,
                    highest.map(|y| y - 12).filter(|y| *y >= file.min_y),
                    ground(top.min(settings.sea_level + 6)),
                ];
                let (x, z) = (volume.min_x + in_x as i32, volume.min_z + in_z as i32);
                for y in asked {
                    for under_fluid in [false, true] {
                        places.push(y.map(|y| (x, y, z, under_fluid)));
                    }
                }
            }
        }
        assert_eq!(
            written.top.len(),
            places.len(),
            "chunk {} {}: the program wrote an answer for each",
            written.x,
            written.z
        );
        let asked: Vec<(i32, i32, i32, bool)> = places.iter().flatten().copied().collect();
        let mut given = terrain
            .top_materials(&mut made, &stored, &asked)
            .into_iter();
        for (place, (at, theirs)) in places.iter().zip(&written.top).enumerate() {
            let mut ours = at.map(|_| given.next().expect("an answer for each block"));
            let mut of_top = &stored;
            // Where the game's source answered otherwise, the answer is made again
            // with that answer.
            let otherwise: Vec<_> = written
                .top_asked
                .iter()
                .filter(|(answer, _)| *answer == place)
                .collect();
            let with;
            if let (false, Some((x, y, z, under_fluid))) = (otherwise.is_empty(), *at) {
                let mut cells = stored.clone();
                for (_, (x, y, z, biome)) in otherwise {
                    cells.set_cell(*x, *y, *z, *biome);
                }
                ours = Some(terrain.top_material(&mut made, &cells, x, y, z, under_fluid));
                with = cells;
                of_top = &with;
            }
            counts.top_answers += 1;
            if let Some(Some(state)) = theirs {
                *counts.top_answered.entry(block_name(*state)).or_default() += 1;
            }
            // The answers come in pairs: without a fluid above, and with one.
            if place % 2 == 1 && written.top[place - 1] != *theirs {
                counts.top_answers_a_fluid_changes += 1;
            }
            if ours != *theirs {
                counts.top_answers_different += 1;
                if counts.top_answers_different <= 6 {
                    find(format!(
                        "chunk {} {}: the top material at {at:?} (the block, and whether a \
                         fluid is above) in {}: {:?} here, {:?} in the game",
                        written.x,
                        written.z,
                        at.map_or("no biome", |(x, y, z, _)| biome_name(of_top.at_block(
                            terrain.zoom(),
                            x,
                            y,
                            z
                        ))),
                        ours.map(|given| given.map(block_name)),
                        theirs.map(|given| given.map(block_name)),
                    ));
                }
            }
        }

        // The mask.
        let mask: Mask = terrain.carving_mask_with(written.x, written.z, biome_of_source);
        let ours: Vec<(usize, usize)> = mask.runs().collect();
        if (mask.min_y(), mask.max_y()) != written.mask_heights || ours != written.mask {
            counts.masks_different += 1;
            let first = ours
                .iter()
                .zip(&written.mask)
                .find(|(ours, theirs)| ours != theirs);
            find(format!(
                "chunk {} {}: the mask is not the game's: {} runs here, {} in the game; the \
                 first that differ: {first:?}",
                written.x,
                written.z,
                ours.len(),
                written.mask.len()
            ));
        }
        counts.reached += mask.count();
        counts.chunks_reached += usize::from(!mask.is_empty());
        let theirs_names = |x: usize, y: i32, z: usize| {
            let index = mask.index(x, y, z);
            (written.mask_heights.0..=written.mask_heights.1).contains(&y)
                && written
                    .mask
                    .iter()
                    .any(|(first, last)| (*first..=*last).contains(&index))
        };

        // The blocks.
        let asked = terrain.apply_carving_mask(
            &mut made,
            written.x,
            written.z,
            &mask,
            &biomes,
            &mut scratch,
        );
        counts.top_material_asked += asked;
        counts.chunks_with_top_material += usize::from(asked != 0);
        let ours = in_the_volumes_order(&made.blocks, height);
        let mut different = 0;
        let sea_level = settings.sea_level;
        for (index, (our, their)) in ours.iter().zip(&written.blocks).enumerate() {
            let y = file.min_y + (index % height) as i32;
            let (in_x, in_z) = (index / height % 16, index / height / 16);
            let before = surfaced[index];
            let named = mask.get(in_x, y, in_z);
            if named {
                if file.uncarvable.contains(&before) {
                    counts.uncarvable_reached += 1;
                } else if before == *their && !before.is_air() && !before.has_fluid() {
                    counts.reached_and_left += 1;
                }
            }
            if before != *their {
                counts.changed += 1;
                *counts.became.entry(block_name(*their)).or_default() += 1;
                *counts.was.entry(block_name(before)).or_default() += 1;
                counts.fluids_put += usize::from(their.has_fluid());
                if !named {
                    // Only the top material changes a block the mask does not name
                    // (it can also change one it names).
                    *counts
                        .top_material_gave
                        .entry(block_name(*their))
                        .or_default() += 1;
                }
            }
            if our == their {
                continue;
            }
            different += 1;
            if different <= 2 {
                let (x, z) = (volume.min_x + in_x as i32, volume.min_z + in_z as i32);
                let origins =
                    terrain.carved_by(written.x, written.z, biome_of_source, (in_x, y, in_z));
                find(format!(
                    "chunk {} {} block {x} {y} {z} in {}: {} ({}) here, {} ({}) in the game; \
                     after the surface {}; the mask names it here: {named}, in the game: {}; \
                     the block below after the surface: {}; sea level {sea_level}; carved \
                     here by {origins:?}",
                    written.x,
                    written.z,
                    biome_name(biomes.at_block(terrain.zoom(), x, y, z)),
                    block_name(*our),
                    our.0,
                    block_name(*their),
                    their.0,
                    block_name(before),
                    theirs_names(in_x, y, in_z),
                    if index % height == 0 {
                        "none"
                    } else {
                        block_name(surfaced[index - 1])
                    },
                ));
            }
        }
        counts.blocks += ours.len();
        counts.blocks_different += different;
        counts.chunks_with_a_different_block += usize::from(different != 0);

        // The marks.
        counts.marks += written.marks.len();
        counts.marks_again += written
            .marks
            .windows(2)
            .filter(|pair| pair[0] == pair[1])
            .count();
        counts.marks_new += written.marks.len().saturating_sub(marks_before);
        if made.marks != written.marks {
            counts.chunks_whose_marks_differ += 1;
            let first = made
                .marks
                .iter()
                .zip(&written.marks)
                .find(|(ours, theirs)| ours != theirs);
            find(format!(
                "chunk {} {}: {} marks here, {} in the game; the first that differ: {first:?}",
                written.x,
                written.z,
                made.marks.len(),
                written.marks.len()
            ));
        }

        // What reached the chunk.
        let started = terrain.carvers_started(written.x, written.z, biome_of_source);
        counts.started += started.len();
        let mut names: Vec<&'static str> = Vec::new();
        for (origin, reached) in &started {
            if *reached != 0 {
                counts.started_reaching += 1;
                counts.reaching_from_the_rim += usize::from(
                    (origin.source.0 - written.x).abs() == REACH
                        || (origin.source.1 - written.z).abs() == REACH,
                );
                if !names.contains(&origin.carver) {
                    names.push(origin.carver);
                }
            }
        }
        for name in names {
            *counts
                .reaching
                .entry(name.trim_start_matches("minecraft:"))
                .or_default() += 1;
        }
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

        // This crate's own biomes: where a source chunk's is not the program's, and
        // what the whole chunk made with them differs in.
        for across_x in -REACH..=REACH {
            for across_z in -REACH..=REACH {
                let (x, z) = (written.x + across_x, written.z + across_z);
                let (own, theirs) = (source.biome(x * 4, 0, z * 4), biome_of_source(x, z));
                if own != theirs {
                    counts.sources_with_another_biome += 1;
                    let carvers = &clustine_worldgen_data::carvers::OF_BIOME;
                    if carvers[usize::from(own.0)] != carvers[usize::from(theirs.0)] {
                        counts.sources_with_other_carvers += 1;
                    }
                }
            }
        }
        let own = terrain.carved(
            written.x,
            written.z,
            &Biomes::from_source(&source, written.x, written.z),
            &source,
        );
        let changed = own
            .chunk
            .blocks
            .iter()
            .zip(&made.blocks)
            .filter(|(own, given)| own != given)
            .count();
        counts.blocks_own_biomes_change += changed;
        counts.chunks_own_biomes_change += usize::from(changed != 0);
    }
    (counts, findings)
}

fn sorted(counted: &BTreeMap<&'static str, usize>) -> String {
    let mut kinds: Vec<(&str, usize)> = counted
        .iter()
        .map(|(name, count)| (*name, *count))
        .collect();
    kinds.sort_by_key(|(name, count)| (std::cmp::Reverse(*count), *name));
    kinds
        .iter()
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<String>>()
        .join(", ")
}

#[test]
#[ignore = "needs what tests/java/GameCarvers.java wrote, which is not in the repository"]
fn chunks_are_carved_as_the_games_own_classes_carve_them() {
    let Ok(directory) = std::env::var("CLUSTINE_GAME_CARVERS") else {
        println!("skipped: CLUSTINE_GAME_CARVERS names no directory of the program's output");
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
    let mut total = [0_usize; 9];
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let file = read(path);
        let (counts, findings) = judge(&file);
        println!("{name}: seed {} {:?}", file.seed, file.dimension);
        println!(
            "  chunks {}, blocks {}, equal {}, different {} in {} chunks; the step changed {} \
             blocks; surfaces that are not the game's: {}",
            counts.chunks,
            counts.blocks,
            counts.blocks - counts.blocks_different,
            counts.blocks_different,
            counts.chunks_with_a_different_block,
            counts.changed,
            counts.surfaces_different
        );
        println!(
            "  masks that are not the game's: {}; the masks name {} blocks in {} chunks, of \
             which {} are of the tag the carvers leave and {} more stayed ground",
            counts.masks_different,
            counts.reached,
            counts.chunks_reached,
            counts.uncarvable_reached,
            counts.reached_and_left
        );
        println!(
            "  marks {} ({} of them a second time, {} more than after the surface), chunks \
             whose marks differ {}",
            counts.marks, counts.marks_again, counts.marks_new, counts.chunks_whose_marks_differ
        );
        println!(
            "  carved away: {}\n  became: {} ({} of them a fluid)",
            sorted(&counts.was),
            sorted(&counts.became),
            counts.fluids_put
        );
        println!(
            "  the top material was asked {} times in {} chunks; blocks changed that no mask \
             names: {}; it asked {} cells of the biome source, {} of them not what the chunks \
             stored, {} outside the chunk's biomes",
            counts.top_material_asked,
            counts.chunks_with_top_material,
            sorted(&counts.top_material_gave),
            counts.asked_cells,
            counts.asked_cells_not_as_stored,
            counts.asked_cells_outside
        );
        println!(
            "  the top material by itself: {} answers, {} different; {} pairs that a fluid \
             above changes; {} cells it asked are not what the chunks stored; answered: {}",
            counts.top_answers,
            counts.top_answers_different,
            counts.top_answers_a_fluid_changes,
            counts.top_cells_not_as_stored,
            sorted(&counts.top_answered)
        );
        println!(
            "  carvers: {} started around the chunks, {} of them reach their chunk, {} of \
             those from eight chunks away; chunks reached by: {}",
            counts.started,
            counts.started_reaching,
            counts.reaching_from_the_rim,
            sorted(&counts.reaching)
        );
        println!(
            "  with this crate's own biomes: {} source chunks have another biome ({} of them \
             other carvers); the whole step then differs in {} blocks of {} chunks",
            counts.sources_with_another_biome,
            counts.sources_with_other_carvers,
            counts.blocks_own_biomes_change,
            counts.chunks_own_biomes_change
        );
        println!(
            "  where: up to {} chunks from the origin; by the biome at the top of the middle \
             column: {}",
            counts.farthest,
            sorted(&counts.biomes)
        );
        for finding in &findings {
            println!("  DIFFERENT {finding}");
        }
        for (sum, count) in total.iter_mut().zip([
            counts.chunks,
            counts.blocks,
            counts.blocks_different,
            counts.marks,
            counts.changed,
            counts.reached,
            counts.top_material_asked,
            counts.top_answers,
            counts.top_answers_different,
        ]) {
            *sum += count;
        }
        failed |= counts.blocks_different != 0
            || counts.surfaces_different != 0
            || counts.chunks_whose_marks_differ != 0
            || counts.masks_different != 0
            || counts.top_answers_different != 0
            || counts.asked_cells_outside != 0;
    }
    println!(
        "all: chunks {}, blocks {}, equal {}, different {}, marks {}, blocks the step changed \
         {}, blocks the masks name {}, top material asked {}; the top material by itself: \
         {} answers, {} different",
        total[0],
        total[1],
        total[1] - total[2],
        total[2],
        total[3],
        total[4],
        total[5],
        total[6],
        total[7],
        total[8]
    );
    assert!(!failed, "the carving differs from the game's");
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
/// its corners. At the edge the source chunks of a chunk lie beyond it.
const FAR: [(i32, i32); 11] = [
    (62_500, 0),
    (-62_500, 62_500),
    (0, -62_500),
    (625_000, 0),
    (-625_000, -625_000),
    (0, 625_000),
    (EDGE, 0),
    (-EDGE, 3),
    (5, EDGE),
    (EDGE, -EDGE),
    (-EDGE + 8, EDGE - 8),
];

/// What a list looks for by running this crate's carvers: a name, and whether a
/// chunk is of the kind, by the carvers that reach it and the biome at its middle.
struct Wanted {
    name: &'static str,
    is: fn(&Reached) -> bool,
}

/// What reaches a chunk, for choosing it.
struct Reached {
    /// Blocks reached by a canyon, a cave of each of the two kinds, the Nether's.
    canyon: usize,
    cave: usize,
    deep_cave: usize,
    nether_cave: usize,
    /// The biome of the cell at the chunk's middle near the sea level, by name.
    biome: &'static str,
}

fn reached(terrain: &Terrain, source: &BiomeSource, chunk: (i32, i32)) -> Reached {
    let mut reached = Reached {
        canyon: 0,
        cave: 0,
        deep_cave: 0,
        nether_cave: 0,
        biome: biome_name(source.biome(chunk.0 * 4 + 2, 16, chunk.1 * 4 + 2)),
    };
    for (origin, blocks) in
        terrain.carvers_started(chunk.0, chunk.1, |x, z| source.biome(x * 4, 0, z * 4))
    {
        match origin.carver {
            "minecraft:canyon" => reached.canyon += blocks,
            "minecraft:cave" => reached.cave += blocks,
            "minecraft:cave_extra_underground" => reached.deep_cave += blocks,
            "minecraft:nether_cave" => reached.nether_cave += blocks,
            other => panic!("the carver {other} is not known to the lists"),
        }
    }
    reached
}

fn is_ocean(reached: &Reached) -> bool {
    reached.biome.ends_with("ocean")
}

/// The kinds of chunk a list of a dimension looks for.
fn wanted(dimension: Dimension) -> Vec<Wanted> {
    match dimension {
        Dimension::Overworld => vec![
            Wanted {
                name: "a canyon",
                is: |reached| reached.canyon > 300,
            },
            Wanted {
                name: "a canyon under an ocean",
                is: |reached| reached.canyon > 300 && is_ocean(reached),
            },
            Wanted {
                name: "a canyon and caves",
                is: |reached| reached.canyon > 0 && reached.cave > 0 && reached.deep_cave > 0,
            },
            Wanted {
                name: "many caves",
                is: |reached| reached.cave > 3000,
            },
            Wanted {
                name: "deep caves",
                is: |reached| reached.deep_cave > 1500,
            },
            Wanted {
                name: "caves under an ocean",
                is: |reached| reached.cave > 800 && is_ocean(reached),
            },
            Wanted {
                name: "caves under a river, a swamp or a beach",
                is: |reached| {
                    reached.cave > 500
                        && ["river", "frozen_river", "swamp", "mangrove_swamp", "beach"]
                            .contains(&reached.biome)
                },
            },
            Wanted {
                name: "caves under mushroom fields",
                is: |reached| reached.cave > 0 && reached.biome == "mushroom_fields",
            },
            Wanted {
                name: "caves under a desert or badlands",
                is: |reached| {
                    reached.cave > 500
                        && ["desert", "badlands", "eroded_badlands", "wooded_badlands"]
                            .contains(&reached.biome)
                },
            },
            Wanted {
                name: "caves under snow or peaks",
                is: |reached| {
                    reached.cave > 500
                        && [
                            "snowy_plains",
                            "snowy_slopes",
                            "frozen_peaks",
                            "jagged_peaks",
                            "stony_peaks",
                            "grove",
                            "ice_spikes",
                            "frozen_ocean",
                        ]
                        .contains(&reached.biome)
                },
            },
            Wanted {
                name: "a few blocks",
                is: |reached| {
                    (1..60).contains(&(reached.cave + reached.deep_cave + reached.canyon))
                },
            },
        ],
        Dimension::Nether => vec![
            Wanted {
                name: "many caves",
                is: |reached| reached.nether_cave > 6000,
            },
            Wanted {
                name: "caves in the basalt deltas",
                is: |reached| reached.nether_cave > 500 && reached.biome == "basalt_deltas",
            },
            Wanted {
                name: "caves in the soul sand valley",
                is: |reached| reached.nether_cave > 500 && reached.biome == "soul_sand_valley",
            },
            Wanted {
                name: "caves in a forest",
                is: |reached| {
                    reached.nether_cave > 500
                        && ["crimson_forest", "warped_forest"].contains(&reached.biome)
                },
            },
            Wanted {
                name: "a few blocks",
                is: |reached| (1..60).contains(&reached.nether_cave),
            },
        ],
        Dimension::End => Vec::new(),
    }
}

/// For each kind a list looked for, how many chunks of it were found.
type Found = Vec<(&'static str, usize)>;

/// The chunks of a list: `count` of them, the same for a seed and dimension every
/// time, and for each kind that was looked for how many were found.
fn list(seed: i64, dimension: Dimension, count: usize) -> (Vec<(i32, i32)>, Found) {
    let terrain = Terrain::new(seed, dimension);
    let source = BiomeSource::new(seed, dimension);
    let mut sequence = Sequence(seed as u64 ^ dimension as u64 ^ 0x6u64);
    let mut chunks: Vec<(i32, i32)> = Vec::new();
    let add = |chunks: &mut Vec<(i32, i32)>, chunk: (i32, i32)| {
        if !chunks.contains(&chunk) && chunks.len() < count {
            chunks.push(chunk);
        }
    };

    add(&mut chunks, (0, 0));
    add(&mut chunks, (-1, -1));

    // Kinds of carving looked for by running this crate's carvers, which says
    // nothing of whether they are right: the game makes the chunks.
    let wanted = wanted(dimension);
    let each = if wanted.is_empty() {
        0
    } else {
        count * 3 / 5 / wanted.len()
    };
    let mut found = vec![Vec::new(); wanted.len()];
    for attempt in 0..40 * count {
        if found
            .iter()
            .all(|of_kind: &Vec<(i32, i32)>| of_kind.len() >= each)
        {
            break;
        }
        let reach = if attempt < 20 * count { 3000 } else { 30_000 };
        let chunk = sequence.chunk((0, 0), reach);
        let reached = reached(&terrain, &source, chunk);
        for (place, kind) in wanted.iter().enumerate() {
            if found[place].len() < each && (kind.is)(&reached) {
                found[place].push(chunk);
            }
        }
    }
    for round in 0..each {
        for of_kind in &found {
            if let Some(chunk) = of_kind.get(round) {
                add(&mut chunks, *chunk);
            }
        }
    }

    // Far out: each far place and chunks around it, also across the edge.
    for far in FAR {
        add(&mut chunks, far);
        for _ in 0..count / 100 {
            let chunk = sequence.chunk(far, 8);
            add(
                &mut chunks,
                (chunk.0.clamp(-EDGE, EDGE), chunk.1.clamp(-EDGE, EDGE)),
            );
        }
    }
    // Squares of four by four: neighbours share their source chunks.
    for _ in 0..count / 100 {
        let corner = sequence.chunk((0, 0), 3000);
        for x in 0..4 {
            for z in 0..4 {
                add(&mut chunks, (corner.0 + x, corner.1 + z));
            }
        }
    }
    // And anywhere within 48,000 blocks.
    while chunks.len() < count {
        add(&mut chunks, sequence.chunk((0, 0), 3000));
    }
    let kinds = wanted
        .iter()
        .zip(&found)
        .map(|(kind, of_kind)| (kind.name, of_kind.len()))
        .collect();
    (chunks, kinds)
}

#[test]
#[ignore = "writes the lists of chunks for tests/java/GameCarvers.java"]
fn lists_of_chunks_for_the_game_to_carve() {
    let Ok(directory) = std::env::var("CLUSTINE_GAME_CARVERS_LISTS") else {
        println!("skipped: CLUSTINE_GAME_CARVERS_LISTS names no directory to write the lists to");
        return;
    };
    std::fs::create_dir_all(&directory).unwrap();
    for (place, seed) in SEEDS.iter().enumerate() {
        for (dimension, name, of_the_first_seed, of_the_others) in [
            (Dimension::Overworld, "overworld", 2400, 300),
            (Dimension::Nether, "nether", 1000, 300),
            (Dimension::End, "end", 100, 40),
        ] {
            let count = if place == 0 {
                of_the_first_seed
            } else {
                of_the_others
            };
            let (chunks, kinds) = list(*seed, dimension, count);
            assert_eq!(chunks.len(), count);
            let text: String = chunks.iter().map(|(x, z)| format!("{x} {z}\n")).collect();
            let path = Path::new(&directory).join(format!("{name}_{seed}.txt"));
            std::fs::write(&path, text).unwrap();
            println!(
                "{}: {count} chunks; found by kind: {kinds:?}",
                path.display()
            );
        }
    }
}

#[test]
fn the_lists_are_the_same_every_time() {
    let (first, kinds) = list(13579, Dimension::Nether, 30);
    assert_eq!(first, list(13579, Dimension::Nether, 30).0);
    assert_eq!(first.len(), 30);
    assert_eq!(first[0], (0, 0));
    assert_eq!(kinds.len(), 5);
}
