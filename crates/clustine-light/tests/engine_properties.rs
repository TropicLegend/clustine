//! The incremental engine against the oracle (`docs/groundwork/n8-plan.md`, section 10,
//! part A): after any sequence of arrivals, adoptions, replacements, departures and block
//! changes, drawn at random from a seed, every present chunk's levels equal the oracle's at
//! every block, and `Lighting::light` equals `clustine_light::light` of the present nine.
//!
//! The worlds are small (four or five chunks a side, three to five sections high) so that
//! the oracle, which lights whole, is quick in a build without optimisation. Their blocks
//! are drawn from those light tells apart: blocks that give light, that stop it, that
//! dampen it by one, slabs and stairs with faces, leaves, water and air.
//!
//! `CLUSTINE_LIGHT_CASES` sets the number of cases of each property (by default 120).

use std::collections::BTreeMap;

use clustine_data::{BlockState, blocks};
use clustine_light::engine::{Chunks, Kept, Lighting, Oracle};
use clustine_light::{Around, light};
use clustine_world::{Biome, BlockPos, Chunk, ChunkPos, Light, Section};

/// A generator of numbers that is the same everywhere.
struct Numbers(u64);

impl Numbers {
    fn new(seed: u64) -> Numbers {
        let mut numbers = Numbers(seed ^ 0x9e37_79b9_7f4a_7c15);
        numbers.below(2);
        numbers
    }

    fn below(&mut self, bound: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % bound
    }

    fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len() as u32) as usize]
    }
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// The blocks a change draws from, light's kinds each.
fn palette() -> Vec<BlockState> {
    vec![
        blocks::AIR,
        blocks::AIR,
        blocks::AIR,
        blocks::STONE,
        blocks::STONE,
        blocks::GLASS,
        blocks::WATER,
        blocks::TORCH,
        blocks::GLOWSTONE,
        state("minecraft:oak_slab[type=bottom]"),
        state("minecraft:oak_slab[type=top]"),
        state("minecraft:oak_stairs[facing=east,half=bottom,shape=straight]"),
        state("minecraft:oak_stairs[facing=north,half=top,shape=straight]"),
        state("minecraft:oak_leaves[distance=1,persistent=true,waterlogged=false]"),
        state("minecraft:sea_lantern"),
        state("minecraft:redstone_lamp[lit=true]"),
        state("minecraft:lantern[hanging=false,waterlogged=false]"),
        state("minecraft:ice"),
        // Blocks that give light and stop it by their shape.
        state("minecraft:enchanting_table"),
        state("minecraft:end_portal_frame[eye=true,facing=north]"),
    ]
}

/// The world: every chunk it has, present to the engine or not.
struct World {
    chunks: BTreeMap<ChunkPos, Chunk>,
    width: i32,
    min_y: i32,
    sections: usize,
}

impl Chunks for World {
    fn chunk(&self, at: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&at)
    }
}

impl World {
    fn new(numbers: &mut Numbers, width: i32, sections: usize, min_y: i32) -> World {
        let mut world = World {
            chunks: BTreeMap::new(),
            width,
            min_y,
            sections,
        };
        // From chunk -2, so that the world lies across both signs of x and z.
        for x in 0..width {
            for z in 0..width {
                let chunk = world.made(numbers);
                world.chunks.insert(ChunkPos::new(x - 2, z - 2), chunk);
            }
        }
        world
    }

    fn height(&self) -> i32 {
        (self.sections * 16) as i32
    }

    /// A chunk of ground with caves, things on it and things floating, some of its
    /// sections of one state throughout.
    fn made(&self, numbers: &mut Numbers) -> Chunk {
        let palette = palette();
        let sections = (0..self.sections)
            .map(|index| {
                let state = if index == 0 && numbers.chance(50) {
                    blocks::STONE
                } else {
                    blocks::AIR
                };
                Section::filled(state, Biome(0))
            })
            .collect();
        let mut chunk = Chunk::from_sections(self.min_y, sections);
        let height = self.height();
        let ground = numbers.below(height as u32 / 2) as i32 + 4;
        for x in 0..16 {
            for z in 0..16 {
                let top = ground + numbers.below(3) as i32;
                for y in 0..top.min(height) {
                    // Caves: air in places under the ground.
                    let state = if numbers.chance(12) {
                        blocks::AIR
                    } else {
                        blocks::STONE
                    };
                    chunk.set(x, self.min_y + y, z, state);
                }
            }
        }
        // A roof floating over part of the chunk, now and then.
        if numbers.chance(40) {
            let y = self.min_y + (ground + 4 + numbers.below(8) as i32).min(height - 1);
            let roof = numbers.pick(&palette);
            for x in 0..numbers.below(16) as usize {
                for z in 0..numbers.below(16) as usize {
                    chunk.set(x, y, z, roof);
                }
            }
        }
        for _ in 0..numbers.below(40) {
            let (x, z) = (numbers.below(16) as usize, numbers.below(16) as usize);
            let y = self.min_y + numbers.below(height as u32) as i32;
            chunk.set(x, y, z, numbers.pick(&palette));
        }
        chunk
    }

    /// A block anywhere in the chunk, but most often by a face or a corner of it.
    fn place(&self, numbers: &mut Numbers, at: ChunkPos) -> BlockPos {
        let coordinate = |numbers: &mut Numbers| match numbers.below(4) {
            0 => 0,
            1 => 15,
            2 => numbers.pick(&[1, 14]),
            _ => numbers.below(16) as i32,
        };
        let x = coordinate(numbers);
        let z = coordinate(numbers);
        let y = self.min_y + numbers.below(self.height() as u32) as i32;
        BlockPos::new(at.x * 16 + x, y, at.z * 16 + z)
    }

    fn any_chunk(&self, numbers: &mut Numbers) -> ChunkPos {
        ChunkPos::new(
            numbers.below(self.width as u32) as i32 - 2,
            numbers.below(self.width as u32) as i32 - 2,
        )
    }
}

/// Says what the engine is told, where `CLUSTINE_LIGHT_TRACE` is set: to find what a
/// failing case did.
fn trace(what: std::fmt::Arguments<'_>) {
    if std::env::var_os("CLUSTINE_LIGHT_TRACE").is_some() {
        eprintln!("{what}");
    }
}

/// The oracle and the engine, told the same.
struct Both {
    engine: Lighting,
    oracle: Oracle,
    sky: bool,
    runs: usize,
}

impl Both {
    fn new(sky: bool) -> Both {
        Both {
            engine: Lighting::new(sky),
            oracle: Oracle::new(sky),
            sky,
            runs: 0,
        }
    }

    fn arrived(&mut self, at: ChunkPos) {
        trace(format_args!("arrived {at:?}"));
        self.engine.arrived(at);
        self.oracle.arrived(at);
    }

    fn left(&mut self, at: ChunkPos) {
        trace(format_args!("left {at:?}"));
        self.engine.left(at);
        self.oracle.left(at);
    }

    fn changed(&mut self, at: BlockPos) {
        trace(format_args!("changed {at:?}"));
        self.engine.changed(at);
        self.oracle.changed(at);
    }

    fn adopt(&mut self, at: ChunkPos, kept: Kept) {
        trace(format_args!("adopt {at:?} {kept:?}"));
        self.engine.adopt(at, kept.clone());
        self.oracle.adopt(at, kept);
    }

    /// Runs both and compares every present chunk.
    fn run(&mut self, world: &World, case: &str) {
        trace(format_args!("run: {case}"));
        self.engine.run(world);
        self.oracle.run(world);
        assert!(self.engine.is_settled(), "{case}: settled after a run");
        self.runs += 1;
        compare(&self.engine, &self.oracle, world, self.sky, self.runs, case);
    }
}

/// Every present chunk of `engine` against the oracle and the crate's function: its light
/// as the function gives it, and its levels at every block as the oracle keeps them,
/// compared whole through what both hand out. One chunk in turn is also read block by
/// block through `at`.
fn compare(engine: &Lighting, oracle: &Oracle, world: &World, sky: bool, turn: usize, case: &str) {
    for (at, chunk) in &world.chunks {
        assert_eq!(engine.has(*at), oracle.has(*at), "{case}: {at:?} present");
        if !engine.has(*at) {
            assert_eq!(engine.light(*at, world), None);
            continue;
        }
        let neighbours = NEIGHBOURS.map(|(dx, dz)| {
            let near = ChunkPos::new(at.x + dx, at.z + dz);
            oracle.has(near).then(|| world.chunks.get(&near)).flatten()
        });
        let expected = light(&Around {
            centre: chunk,
            neighbours,
            sky,
        });
        let made = engine.light(*at, world);
        if made.as_ref() != Some(&expected) {
            panic!(
                "{case}: the light of {at:?} differs from the function's: {}",
                first_difference(made.as_ref(), &expected, world.min_y)
            );
        }
        assert!(engine.is_lit(*at), "{case}: {at:?} lit");
        // The levels of every section, listed or not, through what both hand out.
        let taken = (7, 7);
        let (made, expected) = (engine.kept(*at, taken), oracle.kept(*at, taken));
        let expected = expected.expect("the oracle has lit what is present");
        if made.as_ref().map(|kept| &kept.light) != Some(&expected.light) {
            panic!(
                "{case}: the levels of {at:?} differ from the oracle's: {}",
                first_difference(
                    made.as_ref().map(|kept| &kept.light),
                    &expected.light,
                    world.min_y
                )
            );
        }
        assert_eq!(
            made.map(|kept| kept.walls),
            Some(expected.walls),
            "{case}: walls"
        );
    }
    // One chunk in turn block by block through `at`, and above and below what is kept.
    let present: Vec<&ChunkPos> = world.chunks.keys().filter(|at| engine.has(**at)).collect();
    let Some(at) = present.get(turn % present.len().max(1)) else {
        return;
    };
    for y in world.min_y - 18..world.min_y + world.height() + 18 {
        for z in 0..16 {
            for x in 0..16 {
                let block = BlockPos::new(at.x * 16 + x, y, at.z * 16 + z);
                let (made, expected) = (engine.at(block), oracle.at(block));
                assert!(
                    made == expected,
                    "{case}: at {block:?}, {made:?} where the oracle has {expected:?}"
                );
            }
        }
    }
}

fn first_difference(made: Option<&Light>, expected: &Light, min_y: i32) -> String {
    let Some(made) = made else {
        return "none made".into();
    };
    for (kind, made, expected) in [
        ("sky", &made.sky, &expected.sky),
        ("block", &made.block, &expected.block),
    ] {
        for (index, (made, expected)) in made.iter().zip(expected).enumerate() {
            if made != expected {
                let bottom = min_y + (index as i32 - 1) * 16;
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let read = |light: &clustine_world::SectionLight| match light {
                                clustine_world::SectionLight::Absent => None,
                                clustine_world::SectionLight::Dark => Some(0),
                                clustine_world::SectionLight::Full => Some(15),
                                clustine_world::SectionLight::Levels(levels) => {
                                    let at = y << 8 | z << 4 | x;
                                    Some(levels[at / 2] >> (at % 2 * 4) & 15)
                                }
                            };
                            if read(made) != read(expected) {
                                return format!(
                                    "{kind} light at {x} {} {z} of the chunk: {:?}, not {:?}",
                                    bottom + y as i32,
                                    read(made),
                                    read(expected)
                                );
                            }
                        }
                    }
                }
                return format!("{kind} light of section {index}: listed otherwise");
            }
        }
    }
    "the light says otherwise who made it".into()
}

const NEIGHBOURS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

fn cases() -> u64 {
    std::env::var("CLUSTINE_LIGHT_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(120)
}

/// Levels handed out from a run, waiting to come back with the same blocks.
struct Away {
    kept: Kept,
    /// The blocks changed while it was away: it can only arrive now.
    changed: bool,
}

#[test]
fn any_sequence_of_arrivals_departures_changes_and_adoptions_gives_the_oracles_light() {
    for case in 0..cases() {
        let mut numbers = Numbers::new(case);
        let sky = case % 5 != 4;
        let sections = 3 + numbers.below(3) as usize;
        let min_y = numbers.pick(&[0, -16, -32]);
        let width = 4 + numbers.below(2) as i32;
        let mut world = World::new(&mut numbers, width, sections, min_y);
        let mut both = Both::new(sky);
        let mut away: BTreeMap<ChunkPos, Away> = BTreeMap::new();
        let palette = palette();
        let mut hand_overs = 0;
        // Some chunks to begin with.
        for at in world.chunks.keys() {
            if numbers.chance(60) {
                both.arrived(*at);
            }
        }
        let steps = 10 + numbers.below(10);
        for step in 0..steps {
            let name = format!("case {case} (sky {sky}), step {step}");
            for _ in 0..1 + numbers.below(4) {
                match numbers.below(16) {
                    // Blocks changed, a few in one chunk or one alone.
                    0..=6 => {
                        let at = world.any_chunk(&mut numbers);
                        for _ in 0..1 + numbers.below(6) {
                            let block = world.place(&mut numbers, at);
                            let new = numbers.pick(&palette);
                            let chunk = world.chunks.get_mut(&at).expect("the world has it");
                            chunk.set(
                                (block.x & 15) as usize,
                                block.y,
                                (block.z & 15) as usize,
                                new,
                            );
                            both.changed(block);
                            if let Some(away) = away.get_mut(&at) {
                                away.changed = true;
                            }
                        }
                    }
                    // A chunk arrives.
                    7..=8 => {
                        let at = world.any_chunk(&mut numbers);
                        if !both.engine.has(at) {
                            away.remove(&at);
                            both.arrived(at);
                        }
                    }
                    // A chunk goes.
                    9..=10 => {
                        let at = world.any_chunk(&mut numbers);
                        both.left(at);
                    }
                    // A chunk is replaced whole, present or not.
                    11 => {
                        let at = world.any_chunk(&mut numbers);
                        let chunk = world.made(&mut numbers);
                        world.chunks.insert(at, chunk);
                        away.remove(&at);
                        if both.engine.has(at) || numbers.chance(50) {
                            both.arrived(at);
                        }
                    }
                    // A hand-over: the region runs, hands out a block of chunks with
                    // their levels, and they go.
                    12..=13 => {
                        both.run(&world, &name);
                        hand_overs += 1;
                        let taken = (case, hand_overs);
                        let corner = world.any_chunk(&mut numbers);
                        let (wide, deep) =
                            (1 + numbers.below(3) as i32, 1 + numbers.below(3) as i32);
                        for x in corner.x..corner.x + wide {
                            for z in corner.z..corner.z + deep {
                                let at = ChunkPos::new(x, z);
                                let Some(kept) = both.engine.kept(at, taken) else {
                                    continue;
                                };
                                // The oracle names as walls the chunks that are not
                                // present now, the engine those that were not at the run.
                                assert!(
                                    Some(&kept.light)
                                        == both
                                            .oracle
                                            .kept(at, taken)
                                            .as_ref()
                                            .map(|kept| &kept.light),
                                    "{name}: kept levels of {at:?}"
                                );
                                both.left(at);
                                away.insert(
                                    at,
                                    Away {
                                        kept,
                                        changed: false,
                                    },
                                );
                            }
                        }
                    }
                    // Chunks come back with their levels: all of a hand-over in one
                    // run, or some of them now and some later.
                    _ => {
                        let chosen: Vec<ChunkPos> = away
                            .keys()
                            .copied()
                            .filter(|_| numbers.chance(60))
                            .collect();
                        for at in chosen {
                            let away = away.remove(&at).expect("chosen from those away");
                            if both.engine.has(at) {
                                continue;
                            }
                            if away.changed {
                                both.arrived(at);
                            } else {
                                both.adopt(at, away.kept);
                            }
                        }
                    }
                }
            }
            both.run(&world, &name);
        }
    }
}

#[test]
fn levels_kept_from_one_engine_and_adopted_by_another_give_the_oracles_light() {
    for case in 0..cases() {
        let mut numbers = Numbers::new(1_000_000 + case);
        let sky = case % 4 != 3;
        let sections = 3 + numbers.below(3) as usize;
        let width = 5;
        let mut world = World::new(&mut numbers, width, sections, 0);
        let palette = palette();

        // The region the chunks come from: some of them present, lit, then handed out.
        let mut from = Both::new(sky);
        for at in world.chunks.keys().copied().collect::<Vec<_>>() {
            if numbers.chance(80) {
                from.arrived(at);
            }
        }
        from.run(&world, &format!("case {case}, the first region"));
        // Two hand-overs from it, a tick apart, with a change lit between them.
        let first: Vec<(ChunkPos, Kept)> = world
            .chunks
            .keys()
            .filter_map(|at| Some((*at, from.engine.kept(*at, (1, 1))?)))
            .collect();
        // The first hand-over's levels are of the blocks before the change: the chunk
        // that changed may not take them.
        let changed = world.any_chunk(&mut numbers);
        for _ in 0..1 + numbers.below(4) {
            let block = world.place(&mut numbers, changed);
            let new = numbers.pick(&palette);
            world
                .chunks
                .get_mut(&changed)
                .expect("the world has it")
                .set(
                    (block.x & 15) as usize,
                    block.y,
                    (block.z & 15) as usize,
                    new,
                );
            from.changed(block);
        }
        from.run(&world, &format!("case {case}, the first region again"));
        let second: BTreeMap<ChunkPos, Kept> = world
            .chunks
            .keys()
            .filter_map(|at| Some((*at, from.engine.kept(*at, (1, 2))?)))
            .collect();

        // The region they go to: each chunk adopted from the first hand-over (where its
        // blocks are as they were then), from the second, arrived afresh, or not at
        // all, in one of three runs.
        let mut to = Both::new(sky);
        let mut plan: Vec<(u32, ChunkPos, Option<Kept>)> = Vec::new();
        for (at, kept) in first {
            let run = numbers.below(3);
            let kept = match numbers.below(5) {
                0 if at != changed => Some(kept),
                0..=2 => second.get(&at).cloned(),
                3 => None,
                _ => continue,
            };
            plan.push((run, at, kept));
        }
        for run in 0..3 {
            for (when, at, kept) in &plan {
                if *when != run {
                    continue;
                }
                match kept {
                    Some(kept) => to.adopt(*at, kept.clone()),
                    None => to.arrived(*at),
                }
            }
            to.run(
                &world,
                &format!("case {case}, the second region, run {run}"),
            );
        }
        // The region they went to goes on: a change across a face of an adopted chunk.
        let at = world.any_chunk(&mut numbers);
        for _ in 0..1 + numbers.below(4) {
            let block = world.place(&mut numbers, at);
            let new = numbers.pick(&palette);
            world.chunks.get_mut(&at).expect("the world has it").set(
                (block.x & 15) as usize,
                block.y,
                (block.z & 15) as usize,
                new,
            );
            to.changed(block);
        }
        to.run(
            &world,
            &format!("case {case}, the second region after a change"),
        );
    }
}

/// Three chunks by three of air over a stone floor up to y 4, four sections high.
fn flat() -> World {
    let mut world = World {
        chunks: BTreeMap::new(),
        width: 3,
        min_y: 0,
        sections: 4,
    };
    for x in 0..3 {
        for z in 0..3 {
            let sections = vec![Section::filled(blocks::AIR, Biome(0)); 4];
            let mut chunk = Chunk::from_sections(0, sections);
            for bx in 0..16 {
                for bz in 0..16 {
                    for y in 0..5 {
                        chunk.set(bx, y, bz, blocks::STONE);
                    }
                }
            }
            world.chunks.insert(ChunkPos::new(x, z), chunk);
        }
    }
    world
}

fn put(world: &mut World, both: &mut Both, at: BlockPos, state: BlockState) {
    let chunk = world.chunks.get_mut(&at.chunk()).expect("the world has it");
    chunk.set((at.x & 15) as usize, at.y, (at.z & 15) as usize, state);
    both.changed(at);
}

fn all_arrive(world: &World, both: &mut Both) {
    for at in world.chunks.keys() {
        both.arrived(*at);
    }
}

#[test]
fn a_torch_placed_and_broken_at_a_chunks_corner_lights_and_darkens_all_four() {
    let mut world = flat();
    let mut both = Both::new(true);
    all_arrive(&world, &mut both);
    both.run(&world, "lit");
    // A roof over the corner where four chunks meet, so the torch's light shows.
    for x in 10..22 {
        for z in 10..22 {
            put(&mut world, &mut both, BlockPos::new(x, 9, z), blocks::STONE);
        }
    }
    both.run(&world, "roofed");
    put(
        &mut world,
        &mut both,
        BlockPos::new(15, 5, 15),
        blocks::TORCH,
    );
    both.run(&world, "the torch placed");
    let block = |both: &Both| both.engine.at(BlockPos::new(16, 5, 16)).map(|at| at.block);
    assert_eq!(block(&both), Some(12));
    put(&mut world, &mut both, BlockPos::new(15, 5, 15), blocks::AIR);
    both.run(&world, "the torch broken");
    assert_eq!(block(&both), Some(0));
}

#[test]
fn a_chunk_replaced_whole_takes_the_light_of_its_old_torch_from_its_neighbour() {
    let mut world = flat();
    let mut both = Both::new(true);
    put(
        &mut world,
        &mut both,
        BlockPos::new(31, 5, 24),
        blocks::TORCH,
    );
    all_arrive(&world, &mut both);
    both.run(&world, "lit");
    let block = |both: &Both| both.engine.at(BlockPos::new(32, 5, 24)).map(|at| at.block);
    assert_eq!(block(&both), Some(13));
    // The chunk with the torch comes back with other blocks: stone through.
    let middle = ChunkPos::new(1, 1);
    let stone = vec![Section::filled(blocks::STONE, Biome(0)); 4];
    world.chunks.insert(middle, Chunk::from_sections(0, stone));
    both.arrived(middle);
    both.run(&world, "replaced");
    assert_eq!(block(&both), Some(0));
}

#[test]
fn a_neighbours_own_sky_and_emitter_by_the_face_stay_when_a_chunk_leaves() {
    let mut world = flat();
    let mut both = Both::new(true);
    // A roof over the east chunk with a hole by the face, and a glowstone by the face.
    let east = world.chunks.get_mut(&ChunkPos::new(2, 1)).expect("there");
    for x in 0..16 {
        for z in 0..16 {
            if (x, z) != (0, 4) {
                east.set(x, 9, z, blocks::STONE);
            }
        }
    }
    east.set(0, 5, 10, blocks::GLOWSTONE);
    all_arrive(&world, &mut both);
    both.run(&world, "lit");
    both.left(ChunkPos::new(1, 1));
    both.run(&world, "the middle gone");
    let at = |y, z| both.engine.at(BlockPos::new(32, y, z)).expect("present");
    assert_eq!(at(7, 20).sky, 15);
    assert_eq!(at(5, 26).block, 15);
    assert_eq!(at(6, 26).block, 14);
}

#[test]
fn a_hand_over_adopted_before_with_and_after_its_neighbours_gives_the_same_light() {
    let mut numbers = Numbers::new(77);
    let world = World::new(&mut numbers, 4, 4, 0);
    let mut from = Both::new(true);
    all_arrive(&world, &mut from);
    from.run(&world, "the first region");
    let kept: Vec<(ChunkPos, Kept)> = world
        .chunks
        .keys()
        .map(|at| (*at, from.engine.kept(*at, (3, 3)).expect("lit")))
        .collect();
    // All in one run; one a run before the rest; one a run after; one of each.
    for (early, late) in [
        (None, None),
        (Some(5), None),
        (None, Some(5)),
        (Some(0), Some(15)),
    ] {
        let mut to = Both::new(true);
        let adopt = |to: &mut Both, when: &dyn Fn(usize) -> bool| {
            for (number, (at, kept)) in kept.iter().enumerate() {
                if when(number) {
                    to.adopt(*at, kept.clone());
                }
            }
        };
        adopt(&mut to, &|number| Some(number) == early);
        to.run(&world, "the early one");
        adopt(&mut to, &|number| {
            Some(number) != early && Some(number) != late
        });
        to.run(&world, "the rest");
        adopt(&mut to, &|number| Some(number) == late);
        to.run(&world, "the late one");
        // Two histories to the same blocks: the same levels.
        for (at, _) in &kept {
            assert!(to.engine.kept(*at, (0, 0)) == from.engine.kept(*at, (0, 0)));
        }
    }
}
