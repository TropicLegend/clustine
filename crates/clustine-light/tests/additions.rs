//! What ADR-0023 asks of this crate for an edge, tested from that record (its section
//! 10, and test 1 of its list): which neighbours a block change reaches, a scratch that
//! a call assumes nothing of, and the levels of sections the light leaves out.
//!
//! The worlds are plain arrays of blocks, five chunks by five where the light of the
//! chunks around the middle one is wanted, so that each of those has its own eight.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use clustine_data::{BLOCK_STATE_COUNT, BlockState, blocks};
use clustine_light::{Around, Scratch, Unlisted, UnlistedLevels, light, light_with, reach};
use clustine_world::{
    Biome, Chunk, LIGHT_ARRAY_LENGTH, Light, Section, SectionLight, light_tells_apart,
};

/// The offsets of the eight neighbours in the order [`Around`] and [`reach`] name
/// them: north-west first, row by row. North is negative z.
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

/// A world of air to begin with, from y 0 up, of `reach` chunks to each side of the
/// middle chunk. Blocks are named by `x` and `z` from the middle chunk's north-west
/// corner.
#[derive(Clone)]
struct World {
    reach: i32,
    height: usize,
    states: Vec<BlockState>,
}

impl World {
    fn new(reach: i32, height: usize) -> World {
        let width = (2 * reach as usize + 1) * 16;
        World {
            reach,
            height,
            states: vec![blocks::AIR; width * width * height],
        }
    }

    fn span(&self) -> std::ops::Range<i32> {
        -16 * self.reach..16 * (self.reach + 1)
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        assert!(self.span().contains(&x) && self.span().contains(&z));
        assert!((0..self.height as i32).contains(&y));
        let (low, width) = (self.span().start, self.span().len());
        (y as usize * width + (z - low) as usize) * width + (x - low) as usize
    }

    fn get(&self, x: i32, y: i32, z: i32) -> BlockState {
        self.states[self.index(x, y, z)]
    }

    fn set(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
        let index = self.index(x, y, z);
        self.states[index] = state;
    }

    /// One state in every block of a layer of the whole world.
    fn layer(&mut self, y: i32, state: BlockState) {
        for z in self.span() {
            for x in self.span() {
                self.set(x, y, z, state);
            }
        }
    }

    fn chunk(&self, chunk_x: i32, chunk_z: i32) -> Chunk {
        let sections = (0..self.height as i32 / 16).map(|section| {
            let states: Vec<BlockState> = (0..4096)
                .map(|at| {
                    let (x, y, z) = (at & 15, at >> 8, at >> 4 & 15);
                    self.get(chunk_x * 16 + x, section * 16 + y, chunk_z * 16 + z)
                })
                .collect();
            let states: Box<[BlockState; 4096]> = states.into_boxed_slice().try_into().unwrap();
            Section::from_states(states, Biome(0))
        });
        Chunk::from_sections(0, sections.collect())
    }

    /// All the world's chunks, by their offset from the middle one.
    fn chunks(&self) -> Chunks {
        let mut chunks = BTreeMap::new();
        for chunk_z in -self.reach..=self.reach {
            for chunk_x in -self.reach..=self.reach {
                chunks.insert((chunk_x, chunk_z), self.chunk(chunk_x, chunk_z));
            }
        }
        chunks
    }
}

type Chunks = BTreeMap<(i32, i32), Chunk>;

/// The chunk at an offset from the middle with the eight around it. A chunk the world
/// does not have is not there for light either.
fn around(chunks: &Chunks, chunk_x: i32, chunk_z: i32) -> Around<'_> {
    Around {
        centre: &chunks[&(chunk_x, chunk_z)],
        neighbours: NEIGHBOURS.map(|(dx, dz)| chunks.get(&(chunk_x + dx, chunk_z + dz))),
        sky: true,
    }
}

/// The light of the eight chunks around the middle one, in the order of [`NEIGHBOURS`].
fn light_of_the_eight(chunks: &Chunks, scratch: &mut Scratch) -> Vec<Light> {
    NEIGHBOURS
        .iter()
        .map(|(dx, dz)| light_with(&around(chunks, *dx, *dz), Unlisted::NONE, scratch).0)
        .collect()
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// A generator of numbers that is the same everywhere.
struct Numbers(u64);

impl Numbers {
    fn below(&mut self, bound: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % bound
    }

    fn one_of(&mut self, states: &[BlockState]) -> BlockState {
        states[self.below(states.len() as u32) as usize]
    }
}

// ---------------------------------------------------------------------------------
// Reach.

/// The record's own words for it: `hx + hz <= 14`, where `hx` is 0 for a neighbour in
/// the same line of chunks, `x + 1` for one to the west and `16 - x` for one to the
/// east, and `hz` likewise.
fn within_fourteen_steps(x: usize, z: usize, (dx, dz): (i32, i32)) -> bool {
    let steps = |at: usize, towards: i32| match towards {
        -1 => at + 1,
        0 => 0,
        _ => 16 - at,
    };
    steps(x, dx) + steps(z, dz) <= 14
}

#[test]
fn reach_names_the_neighbours_within_fourteen_steps_of_the_column() {
    let mut columns_reaching = [0; 8];
    let mut reached = 0;
    let mut reaching_a_diagonal = 0;
    for z in 0..16 {
        for x in 0..16 {
            let named = reach(x, z);
            for (index, offset) in NEIGHBOURS.into_iter().enumerate() {
                assert_eq!(
                    named[index],
                    within_fourteen_steps(x, z, offset),
                    "column {x} {z}, neighbour {offset:?}"
                );
                columns_reaching[index] += usize::from(named[index]);
            }
            reached += named.iter().filter(|named| **named).count();
            reaching_a_diagonal += usize::from(named[0] || named[2] || named[5] || named[7]);
        }
    }
    // Each neighbour beside from 14 columns of 16 along its side, each diagonal one
    // from 91 columns, some diagonal one from 244: 5.9 chunks a change at the mean
    // with the chunk itself.
    assert_eq!(columns_reaching, [91, 224, 91, 224, 224, 91, 224, 91]);
    assert_eq!(reaching_a_diagonal, 244);
    assert_eq!(reached, 4 * 224 + 4 * 91);
    assert_eq!((reached + 256) * 10 / 256, 59);
}

#[test]
fn reach_is_towards_the_west_for_a_low_x_and_towards_the_north_for_a_low_z() {
    // The north-west corner: three neighbours are a step or two away, the others 16.
    assert_eq!(
        reach(0, 0),
        [true, true, false, true, false, false, false, false]
    );
    assert_eq!(
        reach(15, 0),
        [false, true, true, false, true, false, false, false]
    );
    assert_eq!(
        reach(0, 15),
        [false, false, false, true, false, true, true, false]
    );
    // Fourteen steps reach and fifteen do not: the west is `x + 1` away.
    assert!(reach(13, 8)[3]);
    assert!(!reach(14, 8)[3]);
    assert!(reach(2, 8)[4]);
    assert!(!reach(1, 8)[4]);
    // A diagonal neighbour is the sum of the two away.
    assert!(reach(6, 6)[0]);
    assert!(!reach(7, 6)[0]);
}

#[test]
#[should_panic(expected = "within it")]
fn reach_refuses_a_column_that_is_not_of_a_chunk() {
    reach(16, 0);
}

/// For each column of the middle chunk: makes the change there, lights the eight
/// neighbours and holds them against their light without it. A neighbour's light
/// differs exactly if `reach` names it.
fn assert_a_change_in_each_column_alters_exactly_what_reach_names(
    world: &World,
    y: i32,
    change: BlockState,
    what: &str,
) {
    let mut chunks = world.chunks();
    let mut scratch = Scratch::new();
    let before = light_of_the_eight(&chunks, &mut scratch);
    for z in 0..16 {
        for x in 0..16 {
            let was = world.get(x as i32, y, z as i32);
            let centre = chunks.get_mut(&(0, 0)).unwrap();
            let sections_with_blocks = sections_holding_a_block(centre);
            centre.set(x, y, z, change);
            assert_eq!(
                sections_holding_a_block(centre),
                sections_with_blocks,
                "the change is to be seen by distance alone"
            );
            let after = light_of_the_eight(&chunks, &mut scratch);
            let named = reach(x, z);
            for index in 0..8 {
                assert_eq!(
                    after[index] != before[index],
                    named[index],
                    "{what} in column {x} {z}: the neighbour {:?}",
                    NEIGHBOURS[index]
                );
            }
            chunks.get_mut(&(0, 0)).unwrap().set(x, y, z, was);
        }
    }
}

fn sections_holding_a_block(chunk: &Chunk) -> Vec<bool> {
    chunk
        .sections()
        .iter()
        .map(|section| section.non_air_count() > 0)
        .collect()
}

#[test]
fn the_brightest_block_alters_the_light_of_exactly_the_neighbours_reach_names() {
    // A floor, so that every section holds a block already, and open air above it.
    let mut world = World::new(2, 16);
    world.layer(0, blocks::STONE);
    assert_eq!(blocks::GLOWSTONE.props().light_emission(), 15);
    assert_a_change_in_each_column_alters_exactly_what_reach_names(
        &world,
        1,
        blocks::GLOWSTONE,
        "glowstone",
    );
}

#[test]
fn a_hole_in_a_roof_alters_the_light_of_exactly_the_neighbours_reach_names() {
    // Dark between a floor and a roof over everything; the sky comes in by the hole.
    let mut world = World::new(2, 16);
    world.layer(0, blocks::STONE);
    world.layer(12, blocks::STONE);
    assert_a_change_in_each_column_alters_exactly_what_reach_names(
        &world,
        12,
        blocks::AIR,
        "a hole in the roof",
    );
}

/// A world with much in its lowest section, little in the next and nothing above.
fn random_world(reach: i32, seed: u64, palette: &[BlockState]) -> World {
    let mut numbers = Numbers(seed);
    let mut world = World::new(reach, 48);
    for index in 0..world.states.len() {
        let y = index / (world.span().len() * world.span().len());
        let of_a_thousand = match y {
            0..16 => 500,
            16..32 => 30,
            _ => 0,
        };
        if numbers.below(1000) < of_a_thousand {
            world.states[index] = numbers.one_of(palette);
        }
    }
    world
}

/// States of every kind light knows: what it passes, what stops it, what dampens it,
/// what stops it by a face, what gives it; and pairs it cannot tell apart.
fn palette() -> Vec<BlockState> {
    vec![
        blocks::AIR,
        blocks::CAVE_AIR,
        blocks::STONE,
        blocks::STONE,
        blocks::COAL_ORE,
        blocks::IRON_ORE,
        blocks::DEEPSLATE,
        blocks::DIRT,
        blocks::GRASS_BLOCK,
        blocks::GLASS,
        blocks::WATER,
        blocks::OAK_LEAVES,
        state("minecraft:oak_slab[type=bottom]"),
        state("minecraft:oak_slab[type=top]"),
        state("minecraft:oak_stairs[facing=east,half=bottom]"),
        state("minecraft:snow[layers=3]"),
        blocks::TORCH,
        blocks::GLOWSTONE,
    ]
}

#[test]
fn a_random_change_alters_only_what_light_can_see_and_only_where_reach_says() {
    let palette = palette();
    let mut scratch = Scratch::new();
    let (mut unseen, mut by_distance, mut by_a_section, mut neighbours_left_alone) = (0, 0, 0, 0);
    for seed in [11, 12] {
        let world = random_world(2, seed, &palette);
        let mut chunks = world.chunks();
        let before = light_of_the_eight(&chunks, &mut scratch);
        let centre_before = light(&around(&chunks, 0, 0));
        let mut numbers = Numbers(seed * 1000);
        for trial in 0..160 {
            let (x, z) = (numbers.below(16) as usize, numbers.below(16) as usize);
            // Two changes of three where there are blocks.
            let y = if trial % 3 == 0 {
                numbers.below(48)
            } else {
                numbers.below(20)
            } as i32;
            let old = world.get(x as i32, y, z as i32);
            let new = numbers.one_of(&palette);
            if new == old {
                continue;
            }
            let centre = chunks.get_mut(&(0, 0)).unwrap();
            let held = sections_holding_a_block(centre);
            centre.set(x, y, z, new);
            let a_section_filled_or_emptied = sections_holding_a_block(centre) != held;
            let what = format!("seed {seed}, trial {trial}: {old:?} to {new:?} at {x} {y} {z}");

            if !light_tells_apart(old, new) {
                // Cases and levels: the lights are compared whole.
                assert_eq!(light_of_the_eight(&chunks, &mut scratch), before, "{what}");
                assert_eq!(light(&around(&chunks, 0, 0)), centre_before, "{what}");
                unseen += 1;
            } else if a_section_filled_or_emptied {
                // Which sections are listed may have changed in all eight.
                by_a_section += 1;
            } else {
                let named = reach(x, z);
                for (index, (dx, dz)) in NEIGHBOURS.into_iter().enumerate() {
                    if named[index] {
                        continue;
                    }
                    let after = light_with(&around(&chunks, dx, dz), Unlisted::NONE, &mut scratch);
                    assert_eq!(after.0, before[index], "{what}: neighbour {dx} {dz}");
                    neighbours_left_alone += 1;
                }
                by_distance += 1;
            }
            chunks.get_mut(&(0, 0)).unwrap().set(x, y, z, old);
        }
    }
    // The run was of every kind of change.
    assert!(unseen >= 10, "{unseen} changes light does not see");
    assert!(by_distance >= 100, "{by_distance} changes by distance");
    assert!(by_a_section >= 10, "{by_a_section} first or last blocks");
    assert!(neighbours_left_alone >= 300, "{neighbours_left_alone}");
}

#[test]
fn the_first_block_of_a_section_alters_neighbours_that_reach_does_not_name() {
    let mut world = World::new(2, 48);
    world.layer(0, blocks::STONE);
    let mut chunks = world.chunks();
    let mut scratch = Scratch::new();
    let before = light_of_the_eight(&chunks, &mut scratch);
    // The east side of the chunk: the west and its two diagonals are 16 steps away.
    let named = reach(15, 8);
    assert!(!named[0] && !named[3] && !named[5]);
    // The lowest section holds the floor and the one above it is beside it. The
    // highest holds nothing and neither does any beside it, until now.
    chunks
        .get_mut(&(0, 0))
        .unwrap()
        .set(15, 40, 8, blocks::STONE);
    let after = light_of_the_eight(&chunks, &mut scratch);
    for index in 0..8 {
        assert_eq!(before[index].sky[3], SectionLight::Absent);
        assert_eq!(before[index].block[4], SectionLight::Absent);
        assert_ne!(after[index].sky[3], SectionLight::Absent, "{index}");
        assert_ne!(after[index].block[4], SectionLight::Absent, "{index}");
    }
    // And back, with the last block of the section.
    chunks.get_mut(&(0, 0)).unwrap().set(15, 40, 8, blocks::AIR);
    assert_eq!(light_of_the_eight(&chunks, &mut scratch), before);
}

// ---------------------------------------------------------------------------------
// What light can tell apart (`clustine_world::light_tells_apart`), which is what an
// edge asks before it asks `reach`.

#[test]
fn light_tells_apart_what_the_record_says_it_does() {
    let bottom = state("minecraft:oak_slab[type=bottom]");
    let top = state("minecraft:oak_slab[type=top]");
    for (one, other) in [
        (blocks::STONE, blocks::AIR),
        (blocks::AIR, blocks::TORCH),
        (blocks::WATER, blocks::AIR),
        (blocks::GLASS, blocks::STONE),
        (bottom, top),
    ] {
        assert!(light_tells_apart(one, other), "{one:?} and {other:?}");
        assert!(light_tells_apart(other, one), "{other:?} and {one:?}");
    }
    for (one, other) in [
        (blocks::STONE, blocks::COAL_ORE),
        (blocks::STONE, blocks::IRON_ORE),
        (blocks::GRASS_BLOCK, blocks::DIRT),
        (blocks::AIR, blocks::CAVE_AIR),
        (blocks::AIR, blocks::VOID_AIR),
    ] {
        assert!(!light_tells_apart(one, other), "{one:?} and {other:?}");
        assert!(!light_tells_apart(other, one), "{other:?} and {one:?}");
    }
}

/// The states that are not air and that light passes exactly as it passes air: they
/// differ from air in nothing the flood reads of a single block.
fn states_light_passes_as_air() -> Vec<BlockState> {
    let air = blocks::AIR.props();
    (0..BLOCK_STATE_COUNT as u16)
        .map(BlockState)
        .filter(|state| {
            let state = state.props();
            !state.is_air()
                && state.light_emission() == air.light_emission()
                && state.light_dampening() == air.light_dampening()
                && state.occludes() == air.occludes()
                && state.occludes_by_shape() == air.occludes_by_shape()
                && state.occlusion_face_set() == air.occlusion_face_set()
                && state.propagates_skylight_down() == air.propagates_skylight_down()
        })
        .collect()
}

#[test]
fn light_tells_air_from_every_state_that_is_not_air() {
    for state in (0..BLOCK_STATE_COUNT as u16).map(BlockState) {
        assert_eq!(
            light_tells_apart(blocks::AIR, state),
            !state.is_air(),
            "{state:?}"
        );
    }
    // Those that differ from air in nothing else are many, and glass is one.
    let like_air = states_light_passes_as_air();
    println!(
        "{} of {BLOCK_STATE_COUNT} states are not air and are to light as air is; the first: {:?}",
        like_air.len(),
        &like_air[..like_air.len().min(8)]
    );
    assert!(like_air.len() > 100);
    assert!(like_air.contains(&blocks::GLASS));
}

#[test]
fn a_block_that_light_passes_as_air_still_alters_which_sections_are_listed() {
    let like_air = states_light_passes_as_air();
    let mut world = World::new(1, 48);
    world.layer(0, blocks::STONE);
    let mut chunks = world.chunks();
    let before = light(&around(&chunks, 1, 0));
    assert_eq!(before.sky[3], SectionLight::Absent);
    for state in [like_air[0], like_air[like_air.len() - 1], blocks::GLASS] {
        chunks.get_mut(&(0, 0)).unwrap().set(8, 40, 8, state);
        let after = light(&around(&chunks, 1, 0));
        // The levels are what they were, and there are more of them to send.
        assert_eq!(after.sky[3], SectionLight::Full, "{state:?}");
        assert_eq!(after.block[3], SectionLight::Dark, "{state:?}");
        assert_eq!(after.sky[..3], before.sky[..3]);
    }
}

// ---------------------------------------------------------------------------------
// The scratch.

/// A world of three chunks by three, of any height, with blocks all through it.
fn busy_world(seed: u64, height: usize) -> World {
    let palette = palette();
    let mut numbers = Numbers(seed);
    let mut world = World::new(1, height);
    for index in 0..world.states.len() {
        if numbers.below(100) < 45 {
            world.states[index] = numbers.one_of(&palette);
        }
    }
    world
}

#[test]
fn light_with_gives_the_light_that_light_gives_whatever_its_scratch_was_used_for() {
    let mut scratch = Scratch::new();
    // Worlds of other heights one after the other, so that the scratch is now too
    // small, now too large, and holds the levels and the lamps of another world.
    for (seed, height) in [(21, 32), (22, 64), (23, 16), (24, 64), (25, 64), (26, 48)] {
        let mut chunks = busy_world(seed, height).chunks();
        if seed % 2 == 0 {
            // Where a chunk is not there, the scratch holds another world's blocks.
            chunks.remove(&(1, 0));
            chunks.remove(&(-1, -1));
            chunks.remove(&(0, 1));
        }
        let around = around(&chunks, 0, 0);
        let expected = light(&around);
        let fresh = light_with(&around, Unlisted::NONE, &mut Scratch::new());
        let used = light_with(&around, Unlisted::NONE, &mut scratch);
        assert_eq!(fresh.0, expected, "seed {seed}");
        assert_eq!(used.0, expected, "seed {seed}");
        assert!(fresh.1.is_empty() && used.1.is_empty());
        // And a second time in the scratch that has just lit this very chunk.
        assert_eq!(
            light_with(&around, Unlisted::NONE, &mut scratch).0,
            expected
        );
    }
    assert_eq!(
        light_with(
            &around(&busy_world(27, 32).chunks(), 0, 0),
            Unlisted::NONE,
            &mut Scratch::default()
        )
        .0
        .sky
        .len(),
        4
    );
}

#[test]
fn a_scratch_that_a_call_left_half_way_gives_the_light_a_fresh_one_gives() {
    let chunks = busy_world(31, 48).chunks();
    let other = busy_world(32, 48).chunks();
    let expected = light(&around(&chunks, 0, 0));

    // The call reads eight chunks of another world, full of lamps, into the scratch
    // and then panics at the ninth, which is of another height.
    let short = busy_world(33, 16).chunk(0, 0);
    let mut broken = around(&other, 0, 0);
    broken.neighbours[7] = Some(&short);
    let mut scratch = Scratch::new();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        light_with(&broken, Unlisted::NONE, &mut scratch)
    }));
    assert!(outcome.is_err(), "a neighbour of another height is refused");
    let all = Unlisted { sky: !0, block: !0 };
    assert_eq!(
        light_with(&around(&chunks, 0, 0), all, &mut scratch),
        light_with(&around(&chunks, 0, 0), all, &mut Scratch::new())
    );
    assert_eq!(
        light_with(&around(&chunks, 0, 0), Unlisted::NONE, &mut scratch).0,
        expected
    );

    // The same after a whole call for the other world, and a panic at the first
    // chunk, when the scratch is as that call left it.
    light_with(&around(&other, 0, 0), all, &mut scratch);
    broken.neighbours = [Some(&short); 8];
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        light_with(&broken, Unlisted::NONE, &mut scratch)
    }));
    assert!(outcome.is_err());
    assert_eq!(
        light_with(&around(&chunks, 0, 0), Unlisted::NONE, &mut scratch).0,
        expected
    );
}

// ---------------------------------------------------------------------------------
// The levels of sections that the light leaves out.

/// The levels of the middle chunk by the rules as the head of the crate states them,
/// swept over the nine chunks until nothing changes: one level for each block from one
/// section below the world to one above it, by `(y * 16 + z) * 16 + x`. Beyond the nine
/// chunks is a wall. The world may hold no block with faces that stop light.
fn slowly(world: &World, of_sky: bool) -> Vec<u8> {
    assert_eq!(world.reach, 1);
    let tall = world.height as i32 + 32;
    let at = |x: i32, y: i32, z: i32| ((y * 48 + z + 16) * 48 + x + 16) as usize;
    let mut dampening = vec![15u8; (tall * 48 * 48) as usize];
    let mut levels = vec![0u8; dampening.len()];
    let mut fixed = vec![false; dampening.len()];
    for z in -16..32 {
        for x in -16..32 {
            let mut under_the_sky = of_sky;
            for y in (0..tall).rev() {
                let here = if y < 16 || y >= tall - 16 {
                    blocks::AIR
                } else {
                    world.get(x, y - 16, z)
                };
                let props = here.props();
                assert!(
                    !(props.occludes() && props.occludes_by_shape())
                        || props.occlusion_face_set() == 0,
                    "{here:?} has faces that stop light"
                );
                dampening[at(x, y, z)] = props.light_dampening().min(15);
                under_the_sky &= props.light_dampening() == 0;
                if under_the_sky {
                    levels[at(x, y, z)] = 15;
                    fixed[at(x, y, z)] = true;
                } else if !of_sky {
                    levels[at(x, y, z)] = props.light_emission();
                }
            }
        }
    }
    loop {
        let mut changed = false;
        for y in 0..tall {
            for z in -16..32 {
                for x in -16..32 {
                    if fixed[at(x, y, z)] {
                        continue;
                    }
                    let takes = dampening[at(x, y, z)].max(1);
                    let mut level = levels[at(x, y, z)];
                    for (dx, dy, dz) in [
                        (-1, 0, 0),
                        (1, 0, 0),
                        (0, -1, 0),
                        (0, 1, 0),
                        (0, 0, -1),
                        (0, 0, 1),
                    ] {
                        let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                        if !(-16..32).contains(&nx)
                            || !(-16..32).contains(&nz)
                            || !(0..tall).contains(&ny)
                        {
                            continue;
                        }
                        level = level.max(levels[at(nx, ny, nz)].saturating_sub(takes));
                    }
                    if level != levels[at(x, y, z)] {
                        levels[at(x, y, z)] = level;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut centre = Vec::new();
    for y in 0..tall {
        for z in 0..16 {
            for x in 0..16 {
                centre.push(levels[at(x, y, z)]);
            }
        }
    }
    centre
}

/// The case of a light section whose levels the sweep gave: `section` counts from the
/// one below the world.
fn section_of(swept: &[u8], section: usize) -> SectionLight {
    let mut array = Box::new([0u8; LIGHT_ARRAY_LENGTH]);
    for (index, level) in swept[section * 4096..(section + 1) * 4096]
        .iter()
        .enumerate()
    {
        array[index / 2] |= level << (index % 2 * 4);
    }
    SectionLight::of(array)
}

/// Ground in the lowest section and something that floats in the highest of six, with
/// four sections of air between: the two in the middle hold no block and are beside
/// none, so they come out absent.
fn world_with_something_afloat(seed: u64) -> World {
    let ground = [
        blocks::STONE,
        blocks::STONE,
        blocks::GLASS,
        blocks::WATER,
        blocks::OAK_LEAVES,
        blocks::TORCH,
        blocks::GLOWSTONE,
    ];
    let mut numbers = Numbers(seed);
    let mut world = World::new(1, 96);
    for z in world.span() {
        for x in world.span() {
            for y in 0..12 {
                if numbers.below(100) < 40 {
                    world.set(x, y, z, numbers.one_of(&ground));
                }
            }
            // A platform over a part of the world, with holes and with leaves and
            // water in it, so that its shadow has every level.
            if (-6..21).contains(&x) && (3..30).contains(&z) {
                let state = match numbers.below(20) {
                    0 => blocks::AIR,
                    1 => blocks::OAK_LEAVES,
                    2 => blocks::WATER,
                    _ => blocks::STONE,
                };
                world.set(x, 88, z, state);
            }
        }
    }
    world
}

#[test]
fn a_section_that_is_absent_has_the_levels_a_slow_sweep_gives() {
    let mut scratch = Scratch::new();
    for seed in [41, 42] {
        let world = world_with_something_afloat(seed);
        let chunks = world.chunks();
        let around = around(&chunks, 0, 0);
        let plain = light(&around);
        // Light sections are the one below the world, the world's six and the one
        // above. Those of the world's third and fourth section are left out.
        let absent = [3, 4];
        for (index, (sky, block)) in plain.sky.iter().zip(&plain.block).enumerate() {
            assert_eq!(*sky == SectionLight::Absent, absent.contains(&index));
            assert_eq!(*block == SectionLight::Absent, absent.contains(&index));
        }

        let all = Unlisted { sky: !0, block: !0 };
        let (made, unlisted) = light_with(&around, all, &mut scratch);
        assert_eq!(made, plain, "asking more does not change the light");
        for (of_sky, sections, more) in [
            (true, &made.sky, &unlisted.sky),
            (false, &made.block, &unlisted.block),
        ] {
            let swept = slowly(&world, of_sky);
            // The sweep is the rules: the sections the light lists are as it says.
            for (index, section) in sections.iter().enumerate() {
                if !absent.contains(&index) {
                    assert_eq!(*section, section_of(&swept, index), "seed {seed}, {index}");
                }
            }
            // And so are those it leaves out, in ascending order and in their one form.
            let expected: Vec<(usize, SectionLight)> = absent
                .iter()
                .map(|index| (*index, section_of(&swept, *index)))
                .collect();
            assert_eq!(*more, expected, "seed {seed}, sky {of_sky}");
            assert!(more.iter().all(|(_, case)| *case != SectionLight::Absent));
        }
        // Under the platform the sky has levels of its own, which is why an edge has
        // to ask; no block's light comes as far as a section that is left out.
        assert!(
            unlisted
                .sky
                .iter()
                .all(|(_, case)| matches!(case, SectionLight::Levels(_)))
        );
        assert!(
            unlisted
                .block
                .iter()
                .all(|(_, case)| *case == SectionLight::Dark)
        );
    }
}

#[test]
fn only_what_is_asked_and_left_out_is_said() {
    let world = world_with_something_afloat(43);
    let chunks = world.chunks();
    let around = around(&chunks, 0, 0);
    let mut scratch = Scratch::new();
    let all = Unlisted { sky: !0, block: !0 };
    let (plain, everything) = light_with(&around, all, &mut scratch);

    // Nothing asked, nothing said.
    assert_eq!(Unlisted::NONE, Unlisted::default());
    let (light, nothing) = light_with(&around, Unlisted::NONE, &mut scratch);
    assert_eq!(light, plain);
    assert_eq!(nothing, UnlistedLevels::default());
    assert!(nothing.is_empty() && !everything.is_empty());

    // The sky and the blocks are asked about apart, section by section.
    let some = Unlisted {
        sky: 1 << 4,
        block: 1 << 3,
    };
    let (light, said) = light_with(&around, some, &mut scratch);
    assert_eq!(light, plain);
    assert_eq!(said.sky, everything.sky[1..]);
    assert_eq!(said.block, everything.block[..1]);

    // Sections the light lists are not said again, and bits for sections the chunk
    // does not have name nothing.
    let listed_or_beyond = Unlisted {
        sky: !(1 << 3 | 1 << 4),
        block: 1 << 0 | 1 << 7 | 1 << 8 | 1 << 63,
    };
    let (light, said) = light_with(&around, listed_or_beyond, &mut scratch);
    assert_eq!(light, plain);
    assert!(said.is_empty());

    // Without a sky there is no sky light to say.
    let mut dark = around;
    dark.sky = false;
    let (light, said) = light_with(&dark, all, &mut scratch);
    assert!(light.sky.iter().all(|case| *case == SectionLight::Absent));
    assert!(said.sky.is_empty());
    assert_eq!(said.block, everything.block);
}

#[test]
fn what_an_edge_has_listed_before_stays_listed_with_the_levels_of_now() {
    // A tower in the middle chunk up into the fourth section.
    let mut world = World::new(1, 96);
    world.layer(0, blocks::STONE);
    for y in 1..60 {
        world.set(8, y, 8, blocks::STONE);
    }
    world.set(8, 60, 8, blocks::GLOWSTONE);
    let mut chunks = world.chunks();
    let mut scratch = Scratch::new();
    let old = light_with(&around(&chunks, 0, 0), Unlisted::NONE, &mut scratch).0;
    let listed = Unlisted::listed_in(&old);
    // The floor lists three light sections; the tower all up to the one above its top.
    assert_eq!(listed.sky, 0b0011_1111);
    assert_eq!(listed.block, 0b0011_1111);

    // The tower is taken down to the floor. The flood now leaves three sections out.
    for y in 1..=60 {
        chunks.get_mut(&(0, 0)).unwrap().set(8, y, 8, blocks::AIR);
    }
    let (mut new, levels) = light_with(&around(&chunks, 0, 0), listed, &mut scratch);
    assert_eq!(Unlisted::listed_in(&new).sky, 0b0000_0111);
    assert_eq!(
        levels.sky,
        [3, 4, 5].map(|index| (index, SectionLight::Full))
    );
    assert_eq!(
        levels.block,
        [3, 4, 5].map(|index| (index, SectionLight::Dark))
    );

    // Put into the light, they are what a client that has the old light is sent: the
    // lamp's light gone and the tower's shadow with it.
    levels.put_into(&mut new);
    assert_eq!(Unlisted::listed_in(&new), listed);
    assert_ne!(old.block[4], SectionLight::Dark);
    assert_ne!(old.sky[2], SectionLight::Full);
    assert_eq!(new.block[4], SectionLight::Dark);
    assert_eq!(new.sky[2], SectionLight::Full);
    // The sections above were never listed and are not now.
    assert_eq!(new.sky[6..], [SectionLight::Absent, SectionLight::Absent]);
}
