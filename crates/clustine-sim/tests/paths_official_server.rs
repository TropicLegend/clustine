//! Paths node for node against the game's own path finder (`docs/groundwork/n7-brief.md`,
//! sections 3.0.1 and 3.21; `docs/groundwork/n7-plan.md`, section 11.5, N7c): the jar's
//! `PathFinder` is run headless on the same layouts by `pathharness/PathHarness.java`.
//!
//! Ignored, as the other comparisons with the official server are: it needs a JDK on the
//! path and the jar's classes that `cargo datagen` unpacks into `target/datagen`. Run it
//! with `cargo test -p clustine-sim --locked --test paths_official_server -- --ignored`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use clustine_data::{BlockState, PathType, blocks};
use clustine_sim::path_harness::{Case, Found, Kind, Layout, find};
use clustine_world::BlockPos;

/// A named layout and a search on it.
struct Named {
    name: String,
    layout: Layout,
    case: Case,
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("no state {text}"))
}

fn floor() -> Layout {
    let mut layout = Layout {
        blocks: BTreeMap::new(),
        min_y: -64,
        height: 384,
    };
    fill(
        &mut layout,
        (-24, -1, -24),
        (24, -1, 24),
        blocks::GRASS_BLOCK,
    );
    layout
}

fn fill(layout: &mut Layout, from: (i32, i32, i32), to: (i32, i32, i32), block: BlockState) {
    for x in from.0..=to.0 {
        for y in from.1..=to.1 {
            for z in from.2..=to.2 {
                layout.blocks.insert(BlockPos::new(x, y, z), block);
            }
        }
    }
}

fn cow(target: (i32, i32, i32)) -> Case {
    Case {
        kind: Kind::Walk,
        position: [0.5, 0.0, 0.5],
        width: 0.9,
        height: 1.4,
        step_height: 0.6,
        max_fall: 3,
        on_ground: true,
        in_floatable: false,
        // A ground navigation's own: doors passed, none opened, no floating.
        flags: [true, false, false, false],
        malus: Vec::new(),
        target: BlockPos::new(target.0, target.1, target.2),
        range: 16.0,
        reach: 1,
        max_visited: 256,
    }
}

fn chicken(target: (i32, i32, i32)) -> Case {
    Case {
        width: 0.4,
        height: 0.7,
        // A chicken avoids water (its constructor's malus).
        malus: vec![(PathType::Water, 0.0)],
        ..cow(target)
    }
}

fn named(name: &str, layout: Layout, case: Case) -> Named {
    Named {
        name: name.to_owned(),
        layout,
        case,
    }
}

/// The layouts the brief names (3.21).
fn chosen() -> Vec<Named> {
    let mut cases = Vec::new();
    cases.push(named("flat", floor(), cow((6, 0, 3))));
    cases.push(named("flat_far", floor(), cow((14, 0, -9))));

    let mut wall = floor();
    fill(&mut wall, (3, 0, -3), (3, 1, 3), blocks::STONE);
    cases.push(named("wall", wall.clone(), cow((6, 0, 0))));
    let mut wide = floor();
    fill(&mut wide, (3, 0, -7), (3, 1, 7), blocks::STONE);
    cases.push(named("wide_wall", wide, cow((6, 0, 0))));

    let mut step = floor();
    fill(&mut step, (3, 0, -24), (24, 0, 24), blocks::STONE);
    cases.push(named("one_block_step", step.clone(), cow((6, 1, 0))));
    let mut two = floor();
    fill(&mut two, (3, 0, -24), (24, 1, 24), blocks::STONE);
    cases.push(named("two_block_step", two, cow((6, 2, 0))));
    let mut slab = floor();
    fill(
        &mut slab,
        (3, 0, -2),
        (3, 0, 2),
        state("minecraft:oak_slab"),
    );
    fill(&mut slab, (4, 0, -24), (24, 0, 24), blocks::STONE);
    fill(
        &mut slab,
        (4, 1, -24),
        (24, 1, 24),
        state("minecraft:oak_slab"),
    );
    cases.push(named("slabs_up", slab, cow((6, 2, 0))));

    for depth in [3, 4] {
        let mut drop = floor();
        fill(&mut drop, (-24, -1, -24), (2, depth - 1, 24), blocks::STONE);
        let mut case = cow((6, 0, 0));
        case.position = [0.5, f64::from(depth), 0.5];
        cases.push(named(&format!("drop_of_{depth}"), drop, case));
    }

    let mut fence = floor();
    fill(
        &mut fence,
        (3, 0, -4),
        (3, 0, 4),
        state("minecraft:oak_fence[north=true,south=true]"),
    );
    cases.push(named("fence_line", fence.clone(), cow((6, 0, 0))));
    let mut over = cow((6, 0, 0));
    over.flags[3] = true;
    cases.push(named("fence_line_walked_over", fence, over));

    let mut posts = floor();
    fill(&mut posts, (3, 0, -6), (3, 1, -1), blocks::STONE);
    fill(&mut posts, (3, 0, 1), (3, 1, 6), blocks::STONE);
    cases.push(named(
        "gap_for_a_chicken",
        posts.clone(),
        chicken((6, 0, 0)),
    ));
    cases.push(named("gap_for_a_cow", posts, cow((6, 0, 0))));

    let mut pond = floor();
    fill(&mut pond, (2, -1, -8), (4, -1, 8), blocks::WATER);
    cases.push(named("water", pond.clone(), cow((7, 0, 0))));
    let mut floating = cow((7, 0, 0));
    floating.flags[2] = true;
    cases.push(named("water_floating", pond, floating));

    let mut door = floor();
    fill(&mut door, (3, 0, -8), (3, 1, 8), blocks::STONE);
    door.blocks.insert(
        BlockPos::new(3, 0, 0),
        state("minecraft:oak_door[facing=east,half=lower]"),
    );
    door.blocks.insert(
        BlockPos::new(3, 1, 0),
        state("minecraft:oak_door[facing=east,half=upper]"),
    );
    cases.push(named("closed_door", door.clone(), cow((6, 0, 0))));
    let mut opener = cow((6, 0, 0));
    opener.flags[1] = true;
    cases.push(named("closed_door_opened", door, opener));

    let mut rail = floor();
    fill(&mut rail, (2, 0, -6), (2, 0, 6), state("minecraft:rail"));
    cases.push(named("rail", rail, cow((5, 0, 0))));

    let mut ring = floor();
    for (x, z) in [(2, -1), (2, 0), (2, 1), (-1, 2), (0, 2), (1, 2)] {
        ring.blocks.insert(BlockPos::new(x, 0, z), blocks::CACTUS);
    }
    cases.push(named("cactus_ring", ring, cow((5, 0, 5))));

    let mut honey = floor();
    fill(&mut honey, (2, -1, -1), (4, -1, 1), blocks::HONEY_BLOCK);
    cases.push(named("honey", honey, cow((6, 0, 0))));

    let mut boxed = floor();
    fill(&mut boxed, (8, 0, -3), (12, 3, 3), blocks::STONE);
    boxed.blocks.remove(&BlockPos::new(10, 0, 0));
    boxed.blocks.remove(&BlockPos::new(10, 1, 0));
    cases.push(named("unreachable", boxed, cow((10, 0, 0))));
    cases
}

/// A deterministic stream for the random layouts.
struct Stream(u64);

impl Stream {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

/// Plots of rough ground with walls, fences, water, slabs and the blocks that have path
/// types of their own, and a mob of each size and flag searching across them.
fn random(count: u64) -> Vec<Named> {
    let palette: Vec<(BlockState, bool)> = vec![
        (blocks::STONE, true),
        (state("minecraft:oak_fence"), false),
        (state("minecraft:cobblestone_wall"), false),
        (state("minecraft:oak_slab"), false),
        (state("minecraft:oak_slab[type=top]"), false),
        (state("minecraft:oak_stairs[facing=east]"), false),
        (blocks::WATER, false),
        (blocks::LAVA, false),
        (blocks::CACTUS, false),
        (blocks::HONEY_BLOCK, false),
        (blocks::MAGMA_BLOCK, false),
        (blocks::POWDER_SNOW, false),
        (state("minecraft:sweet_berry_bush[age=3]"), false),
        (state("minecraft:oak_trapdoor"), false),
        (
            state("minecraft:oak_trapdoor[open=true,facing=north]"),
            false,
        ),
        (blocks::WHITE_CARPET, false),
        (blocks::GLASS_PANE, false),
        (blocks::SOUL_SAND, false),
        (state("minecraft:campfire[lit=true]"), false),
        (blocks::FIRE, false),
        (blocks::COBWEB, false),
        (blocks::DIRT_PATH, false),
        (blocks::FARMLAND, false),
        (blocks::OAK_LEAVES, false),
        (state("minecraft:rail"), false),
    ];
    let mut stream = Stream(0x05ee_d0f9_a7b5);
    let mut cases = Vec::new();
    for index in 0..count {
        let mut layout = floor();
        let mut heights = BTreeMap::new();
        for x in -10..=10 {
            for z in -10..=10 {
                // Rough ground: a block or two up here and there.
                let height = match stream.below(10) {
                    0 => 1,
                    1 => 2,
                    _ => 0,
                };
                heights.insert((x, z), height);
                if height > 0 {
                    fill(&mut layout, (x, 0, z), (x, height - 1, z), blocks::STONE);
                }
                if stream.below(6) == 0 {
                    let (block, _) = palette[stream.below(palette.len() as u64) as usize];
                    let at = if block == blocks::WATER || block == blocks::LAVA {
                        height - 1
                    } else {
                        height
                    };
                    layout.blocks.insert(BlockPos::new(x, at, z), block);
                }
            }
        }
        let tx = stream.below(17) as i32 - 8;
        let tz = stream.below(17) as i32 - 8;
        let ty = heights[&(tx, tz)];
        let mut case = if stream.below(3) == 0 {
            chicken((tx, ty, tz))
        } else {
            cow((tx, ty, tz))
        };
        // The mob stands on its column, cleared for it.
        let start_height = heights[&(0, 0)];
        for y in start_height..start_height + 3 {
            layout.blocks.remove(&BlockPos::new(0, y, 0));
        }
        case.position = [0.5, f64::from(start_height), 0.5];
        case.flags = [
            stream.below(4) != 0,
            stream.below(3) == 0,
            stream.below(2) == 0,
            stream.below(5) == 0,
        ];
        case.max_visited = [64, 256, 256, 400][stream.below(4) as usize];
        case.kind = match stream.below(10) {
            0 => Kind::Fly,
            1 => Kind::Amphibious,
            _ => Kind::Walk,
        };
        if case.kind == Kind::Fly {
            // In the air above it all, as a parrot or a bee flies.
            case.position[1] += 3.0;
            case.on_ground = false;
        }
        cases.push(named(&format!("random_{index}"), layout, case));
    }
    cases
}

/// Boxes of water with stone, kelp-free, for the swimming evaluator, with a fish's size.
fn underwater(count: u64) -> Vec<Named> {
    let mut stream = Stream(0x000f_154a_7e12);
    let mut cases = Vec::new();
    for index in 0..count {
        let mut layout = floor();
        fill(&mut layout, (-10, 0, -10), (10, 8, 10), blocks::WATER);
        for _ in 0..120 {
            let x = stream.below(21) as i32 - 10;
            let y = stream.below(9) as i32;
            let z = stream.below(21) as i32 - 10;
            if (x, z) != (0, 0) {
                layout.blocks.insert(BlockPos::new(x, y, z), blocks::STONE);
            }
        }
        let target = (
            stream.below(17) as i32 - 8,
            stream.below(9) as i32,
            stream.below(17) as i32 - 8,
        );
        layout
            .blocks
            .insert(BlockPos::new(target.0, target.1, target.2), blocks::WATER);
        let mut case = cow(target);
        case.kind = Kind::Swim;
        case.position = [0.5, 3.0, 0.5];
        case.width = 0.5;
        case.height = 0.3;
        case.on_ground = false;
        case.in_floatable = true;
        case.flags = [true, false, true, false];
        cases.push(named(&format!("underwater_{index}"), layout, case));
    }
    cases
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The jars the game runs on, as `cargo datagen` unpacked them.
fn class_path() -> Vec<PathBuf> {
    let root = workspace().join("target/datagen/26.3/extract/classpath");
    let mut jars = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).unwrap_or_else(|error| {
            panic!(
                "reading {} ({error}): run `cargo datagen` first",
                directory.display()
            )
        });
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "jar") {
                jars.push(path);
            }
        }
    }
    jars.sort();
    assert!(!jars.is_empty(), "no jars in {}", root.display());
    jars
}

fn write_cases(cases: &[Named]) -> String {
    let mut text = String::new();
    for named in cases {
        let case = &named.case;
        let kind = match case.kind {
            Kind::Walk => "walk",
            Kind::Swim => "swim",
            Kind::Fly => "fly",
            Kind::Amphibious => "amphibious",
        };
        let bit = |flag: bool| u8::from(flag);
        writeln!(text, "case {}", named.name).unwrap();
        writeln!(text, "evaluator {kind}").unwrap();
        writeln!(
            text,
            "mob {} {} {} {} {} {} {} {} {}",
            case.position[0],
            case.position[1],
            case.position[2],
            case.width,
            case.height,
            case.step_height,
            case.max_fall,
            bit(case.on_ground),
            bit(case.in_floatable)
        )
        .unwrap();
        let [a, b, c, d] = case.flags;
        writeln!(text, "flags {} {} {} {}", bit(a), bit(b), bit(c), bit(d)).unwrap();
        for (kind, value) in &case.malus {
            writeln!(text, "malus {} {value}", *kind as usize).unwrap();
        }
        writeln!(text, "level {} {}", named.layout.min_y, named.layout.height).unwrap();
        for (pos, block) in &named.layout.blocks {
            writeln!(
                text,
                "fill {} {} {} {} {} {} {}",
                pos.x, pos.y, pos.z, pos.x, pos.y, pos.z, block.0
            )
            .unwrap();
        }
        writeln!(
            text,
            "search {} {} {} {} {} {}",
            case.target.x, case.target.y, case.target.z, case.range, case.reach, case.max_visited
        )
        .unwrap();
        writeln!(text, "end").unwrap();
    }
    text
}

/// The game's answers, by case.
fn read_answers(text: &str) -> BTreeMap<String, Option<Found>> {
    let mut answers = BTreeMap::new();
    let mut current: Option<(String, Option<Found>)> = None;
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[0] {
            "case" => {
                if let Some((name, found)) = current.take() {
                    answers.insert(name, found);
                }
                current = Some((words[1].to_owned(), None));
            }
            "none" => {}
            "path" => {
                let found = Found {
                    reached: words[1] == "1",
                    nodes: Vec::new(),
                };
                current.as_mut().expect("a path in a case").1 = Some(found);
            }
            "node" => {
                let number = |at: usize| words[at].parse::<i32>().expect("a number");
                let kind = PathType::ALL[number(4) as usize];
                let found = current.as_mut().and_then(|(_, found)| found.as_mut());
                found
                    .expect("a node in a path")
                    .nodes
                    .push((BlockPos::new(number(1), number(2), number(3)), kind));
            }
            other => panic!("unknown answer line {other}"),
        }
    }
    if let Some((name, found)) = current {
        answers.insert(name, found);
    }
    answers
}

fn the_games(cases: &[Named]) -> BTreeMap<String, Option<Found>> {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let classes = scratch.path().join("classes");
    let run = scratch.path().join("run");
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&run).unwrap();
    let jar_paths = class_path();
    let jars = env::join_paths(&jar_paths).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pathharness/PathHarness.java");
    let compiled = Command::new("javac")
        .arg("-proc:none")
        .arg("-nowarn")
        .arg("-cp")
        .arg(&jars)
        .arg("-d")
        .arg(&classes)
        .arg(&source)
        .status()
        .expect("starting javac: a JDK has to be on the path");
    assert!(compiled.success(), "compiling the harness failed");

    let input = scratch.path().join("cases.txt");
    let output = scratch.path().join("paths.txt");
    fs::write(&input, write_cases(cases)).unwrap();
    let mut class_path = vec![classes];
    class_path.extend(jar_paths);
    let ran = Command::new("java")
        .arg("-cp")
        .arg(env::join_paths(class_path).unwrap())
        .arg("PathHarness")
        .arg(&input)
        .arg(&output)
        .current_dir(&run)
        .status()
        .expect("starting java");
    assert!(ran.success(), "the harness failed");
    read_answers(&fs::read_to_string(&output).unwrap())
}

fn compare(cases: &[Named]) {
    let games = the_games(cases);
    let mut differ = Vec::new();
    for named in cases {
        let ours = find(&named.layout, &named.case);
        let theirs = games
            .get(&named.name)
            .unwrap_or_else(|| panic!("the harness did not answer {}", named.name));
        if &ours != theirs {
            differ.push(format!(
                "{}:\n  ours:   {ours:?}\n  theirs: {theirs:?}",
                named.name
            ));
        }
    }
    assert!(
        differ.is_empty(),
        "{} of {} paths differ:\n{}",
        differ.len(),
        cases.len(),
        differ.join("\n")
    );
}

#[test]
#[ignore = "needs a JDK and the jar's classes from `cargo datagen`"]
fn official_server_paths_of_the_chosen_layouts_are_the_games_node_for_node() {
    compare(&chosen());
}

#[test]
#[ignore = "needs a JDK and the jar's classes from `cargo datagen`"]
fn official_server_paths_across_random_rough_ground_are_the_games_node_for_node() {
    compare(&random(400));
}

#[test]
#[ignore = "needs a JDK and the jar's classes from `cargo datagen`"]
fn official_server_paths_through_random_water_are_the_games_node_for_node() {
    compare(&underwater(100));
}
