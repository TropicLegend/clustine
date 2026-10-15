//! `Chunk::set` and what follows the blocks, tested from
//! `docs/adr/0022-what-a-chunk-carries.md` (sections 1 and 2 and the second of its
//! tests) through the crate's public interface alone. What is expected is the
//! record's rules, written out again in `common`; nothing is taken from the code.

mod common;

use std::collections::BTreeMap;

use clustine_data::{BlockState, blocks};
use clustine_world::{
    BlockEntity, Chunk, DataRefused, InChunk, MAX_BLOCK_ENTITY_DATA, MAX_CHUNK_ENTITY_DATA, Section,
};
use common::*;

/// The record's bounds, in its own figures.
const SIXTEEN_KIB: usize = 16 * 1024;
const ONE_MIB: usize = 1024 * 1024;

#[test]
fn the_kinds_of_states_are_what_the_tests_take_them_for() {
    let fluid = |state: BlockState| state.props().fluid().is_some();
    assert!(fluid(blocks::WATER) && fluid(flowing_water()) && fluid(blocks::LAVA));
    assert!(fluid(waterlogged_slab()) && fluid(waterlogged_leaves()));
    assert!(!fluid(blocks::OAK_SLAB) && !fluid(blocks::OAK_LEAVES) && !fluid(blocks::STONE));

    // Leaves block motion for one map and not for the one "without leaves"; water
    // for neither tag; a torch for nothing but the world's surface.
    assert_eq!(counts_for(blocks::OAK_LEAVES), [true, true, true, false]);
    assert_eq!(counts_for(waterlogged_leaves()), [true, true, true, true]);
    assert_eq!(counts_for(blocks::WATER), [true, false, true, true]);
    assert_eq!(counts_for(blocks::STONE), [true; 4]);
    assert_eq!(counts_for(blocks::TORCH), [true, false, false, false]);
    assert_eq!(counts_for(blocks::CAVE_AIR), [false; 4]);

    let chest = entity_type(blocks::CHEST).expect("a chest has a block entity");
    assert_eq!(entity_type(chest_facing_east()), Some(chest));
    let others = [
        blocks::TRAPPED_CHEST,
        blocks::OAK_SIGN,
        blocks::FURNACE,
        blocks::BARREL,
    ];
    let mut types = vec![chest];
    for other in others {
        types.push(entity_type(other).expect("it has a block entity"));
    }
    types.sort_unstable();
    types.dedup();
    assert_eq!(types.len(), 5, "five types of block entity among them");
    assert_eq!(entity_type(lit_furnace()), entity_type(blocks::FURNACE));
    for plain in [blocks::STONE, blocks::WATER, blocks::TORCH, blocks::AIR] {
        assert_eq!(entity_type(plain), None);
    }

    // The record's own examples of what light sees and what it does not.
    for (old, new) in [
        (blocks::STONE, blocks::IRON_ORE),
        (blocks::STONE, blocks::COAL_ORE),
        (blocks::GRASS_BLOCK, blocks::DIRT),
        (blocks::OAK_DOOR, powered_door()),
        (blocks::AIR, blocks::CAVE_AIR),
    ] {
        assert!(!light_sees(old, new), "{old:?} and {new:?}");
    }
    for (old, new) in [
        (blocks::AIR, blocks::TORCH),
        (blocks::AIR, blocks::WATER),
        (blocks::AIR, flowing_water()),
        (blocks::GLASS, blocks::STONE),
        (blocks::AIR, blocks::STONE),
        (blocks::FURNACE, lit_furnace()),
    ] {
        assert!(light_sees(old, new), "{old:?} and {new:?}");
    }
}

/// What the record says a chunk is after a sequence of changes, kept beside the chunk
/// in the plainest way: a state for every position, and section 2's five steps.
struct Plain {
    min_y: i32,
    blocks: Vec<BlockState>,
    entities: BTreeMap<InChunk, BlockEntity>,
    generated: u16,
    with_failure: bool,
    lit: bool,
}

impl Plain {
    fn of(chunk: &Chunk) -> Self {
        assert!(chunk.block_entities().is_empty());
        Self {
            min_y: chunk.min_y(),
            blocks: chunk
                .sections()
                .iter()
                .flat_map(|section| section.states())
                .collect(),
            entities: BTreeMap::new(),
            generated: chunk.generated(),
            with_failure: chunk.with_failure(),
            lit: chunk.light().is_some(),
        }
    }

    fn index(&self, x: usize, y: i32, z: usize) -> Option<usize> {
        let up = usize::try_from(y - self.min_y).ok()?;
        (up < self.blocks.len() / 256).then_some(up << 8 | z << 4 | x)
    }

    fn get(&self, x: usize, y: i32, z: usize) -> Option<BlockState> {
        Some(self.blocks[self.index(x, y, z)?])
    }

    /// The highest block of the column that counts for `map`, if any does.
    fn highest(&self, x: usize, z: usize, map: usize) -> Option<i32> {
        let top = self.min_y + (self.blocks.len() / 256) as i32;
        (self.min_y..top)
            .rev()
            .find(|y| counts_for(self.get(x, *y, z).expect("inside"))[map])
    }

    fn set(&mut self, x: usize, y: i32, z: usize, new: BlockState) -> Option<BlockState> {
        let index = self.index(x, y, z)?;
        let old = self.blocks[index];
        if old == new {
            return Some(old);
        }
        self.blocks[index] = new;
        // "If the new block has no block entity type, or another one than the old,
        // the entity at the position is removed; if it has one and none of that type
        // is there, one without data is put there."
        let at = InChunk::new(x, y, z);
        let kind = entity_type(new);
        if kind != entity_type(old) {
            self.entities.remove(&at);
        }
        if let Some(kind) = kind {
            self.entities.entry(at).or_insert(BlockEntity {
                kind,
                data: Vec::new(),
            });
        }
        self.generated = 0;
        self.with_failure = false;
        if light_sees(old, new) {
            self.lit = false;
        }
        Some(old)
    }
}

/// What is cheap enough to ask after every step: the block, the four heights of its
/// column, the block entities, the two things a change clears, and the light.
fn agrees_at(chunk: &Chunk, plain: &Plain, x: usize, y: i32, z: usize, step: usize) {
    assert_eq!(chunk.get(x, y, z), plain.get(x, y, z), "step {step}");
    let kept = kept(chunk);
    for (map, kept) in kept.iter().enumerate() {
        let expected = plain
            .highest(x, z, map)
            .map_or(0, |y| (y - plain.min_y + 1) as u16);
        assert_eq!(
            kept[z << 4 | x],
            expected,
            "step {step}: map {map} at x {x} z {z}"
        );
    }
    assert_eq!(chunk.block_entities(), &plain.entities, "step {step}");
    assert_eq!(chunk.generated(), plain.generated, "step {step}");
    assert_eq!(chunk.with_failure(), plain.with_failure, "step {step}");
    the_failure_bit_is_only_of_a_generated_chunk(chunk);
    // I1, and that a chunk keeps the light it can know to be valid.
    assert_eq!(chunk.light().is_some(), plain.lit, "step {step}: the light");
}

/// Everything, by a scan of the whole chunk.
fn agrees(chunk: &Chunk, plain: &Plain) {
    let states: Vec<BlockState> = chunk
        .sections()
        .iter()
        .flat_map(|section| section.states())
        .collect();
    assert_eq!(states, plain.blocks);
    counts_and_heights_follow_the_blocks(chunk);
    block_entities_follow_the_blocks(chunk);
    assert_eq!(chunk.block_entities(), &plain.entities);
}

/// Changes `chunk` at random for `steps` steps, among them every kind of thing the
/// record's rules speak of, and holds it against the record after each.
fn changes_at_random(mut chunk: Chunk, seed: u64, steps: usize, full_check_every: usize) {
    let kinds = kinds();
    let mut numbers = numbers(seed);
    let mut next = move || numbers.next().expect("it does not end");
    let (ticks, marks) = (some_ticks(), some_marks());
    chunk.set_ticks(ticks.clone());
    chunk.set_marks(marks.clone());
    let marks = chunk.marks().to_vec();
    let mut plain = Plain::of(&chunk);
    let height = chunk.height() as i32;
    // What the run met, so that it is known to have tried what it is for.
    let (mut kept_light, mut lost_light, mut kept_data, mut lowered) = (0, 0, 0, 0);

    for step in 0..steps {
        // Mostly a few columns and a band of a few blocks, so that blocks are put on
        // one another, changed again and taken away from under and above one another.
        let (x, z) = if next() % 5 == 0 {
            (next() % 16, next() % 16)
        } else {
            (next() % 3, next() % 3)
        };
        let y = if next() % 4 == 0 {
            chunk.min_y() + (next() % height as usize) as i32
        } else {
            chunk.min_y() + height / 2 + (next() % 12) as i32
        };
        match next() % 100 {
            // Outside the dimension: nothing is there and nothing changes.
            0..5 => {
                let outside = if next() % 2 == 0 {
                    chunk.min_y() - 1 - (next() % 40) as i32
                } else {
                    chunk.min_y() + height + (next() % 40) as i32
                };
                let before = chunk.clone();
                assert_eq!(chunk.set(x, outside, z, kinds[next() % kinds.len()]), None);
                assert_eq!(chunk.get(x, outside, z), None);
                assert_eq!(chunk, before, "step {step}");
            }
            // The state that is there: not a change.
            5..15 => {
                let there = plain.get(x, y, z).expect("inside");
                let before = chunk.clone();
                assert_eq!(chunk.set(x, y, z, there), Some(there));
                assert_eq!(chunk, before, "step {step}");
                assert_eq!(chunk.generated(), plain.generated);
                assert_eq!(chunk.with_failure(), plain.with_failure);
            }
            // A generator says the chunk is as it made it, and gives it light.
            15..30 => {
                let version = 1 + (next() % 3) as u16;
                let failed = next() % 2 == 0;
                chunk.mark_generated(version, failed);
                chunk.set_light(some_light(&chunk, 1));
                plain.generated = version;
                plain.with_failure = failed;
                plain.lit = true;
            }
            // Data for a block entity that is there.
            30..38 => {
                if !plain.entities.is_empty() {
                    let which = next() % plain.entities.len();
                    let at = *plain.entities.keys().nth(which).expect("that many");
                    let data = vec![(next() % 256) as u8; 1 + next() % 40];
                    assert_eq!(chunk.set_block_entity_data(at, data.clone()), Ok(()));
                    plain.entities.get_mut(&at).expect("there").data = data;
                }
            }
            // The top of a column goes, so that a height has to be found again below.
            38..55 => {
                let map = next() % 4;
                if let Some(top) = plain.highest(x, z, map) {
                    let gone = [blocks::AIR, blocks::CAVE_AIR, blocks::WATER][next() % 3];
                    let old = plain.set(x, top, z, gone);
                    assert_eq!(chunk.set(x, top, z, gone), old, "step {step}");
                    lowered += 1;
                    agrees_at(&chunk, &plain, x, top, z, step);
                }
            }
            // A block with a block entity becomes another state of the same type: a
            // chest is turned, a furnace is lit. Its entity stays as it is.
            55..62 => {
                if !plain.entities.is_empty() {
                    let which = next() % plain.entities.len();
                    let at = *plain.entities.keys().nth(which).expect("that many");
                    let (x, y, z) = (usize::from(at.x), i32::from(at.y), usize::from(at.z));
                    let there = plain.get(x, y, z).expect("inside");
                    let other = kinds
                        .iter()
                        .find(|kind| **kind != there && entity_type(**kind) == entity_type(there));
                    if let Some(other) = other {
                        let before = plain.entities[&at].clone();
                        kept_data += usize::from(!before.data.is_empty());
                        assert_eq!(plain.set(x, y, z, *other), Some(there));
                        assert_eq!(chunk.set(x, y, z, *other), Some(there), "step {step}");
                        assert_eq!(chunk.block_entities().get(&at), Some(&before));
                        agrees_at(&chunk, &plain, x, y, z, step);
                    }
                }
            }
            // Any state anywhere.
            _ => {
                let new = kinds[next() % kinds.len()];
                let was_lit = plain.lit;
                let had_data = plain
                    .entities
                    .get(&InChunk::new(x, y, z))
                    .is_some_and(|entity| !entity.data.is_empty());
                let old = plain.set(x, y, z, new);
                assert_eq!(chunk.set(x, y, z, new), old, "step {step}");
                if was_lit && old != Some(new) {
                    kept_light += usize::from(plain.lit);
                    lost_light += usize::from(!plain.lit);
                }
                if had_data && old != Some(new) && entity_type(new) == old.and_then(entity_type) {
                    kept_data += 1;
                }
            }
        }
        agrees_at(&chunk, &plain, x, y, z, step);
        assert_eq!(chunk.ticks(), ticks, "step {step}: ticks are untouched");
        assert_eq!(chunk.marks(), marks, "step {step}: marks are untouched");
        if step % full_check_every == 0 {
            agrees(&chunk, &plain);
        }
    }
    agrees(&chunk, &plain);
    assert!(
        kept_light > 20 && lost_light > 20 && kept_data > 5 && lowered > 100,
        "the run tried too little: light kept {kept_light} times and lost {lost_light}, \
         data kept {kept_data} times, a column lowered {lowered} times"
    );
}

#[test]
fn after_random_changes_a_small_chunk_is_what_the_record_says() {
    for seed in [1, 2, 3] {
        changes_at_random(of_air(-16, 4), seed, 6000, 97);
    }
}

#[test]
fn after_random_changes_a_chunk_of_the_overworld_is_what_the_record_says() {
    let chunk = Chunk::empty(overworld(), PLAINS);
    assert_eq!((chunk.min_y(), chunk.height()), (-64, 384));
    changes_at_random(chunk, 4, 6000, 997);
}

#[test]
fn after_random_changes_a_chunk_that_began_full_of_stone_is_what_the_record_says() {
    // Heights that start at the very top, and sections that start uniform and solid.
    let chunk = Chunk::from_sections(0, vec![Section::filled(blocks::STONE, PLAINS); 3]);
    counts_and_heights_follow_the_blocks(&chunk);
    changes_at_random(chunk, 5, 6000, 97);
}

#[test]
fn a_chunk_made_from_sections_has_the_heights_counts_and_block_entities_of_its_blocks() {
    let kinds = kinds();
    let mut numbers = numbers(9);
    let mut sections = Vec::new();
    for index in 0..5 {
        let mut states = Box::new([blocks::AIR; 4096]);
        // The topmost section stays air, so that heights lie below the top.
        if index < 4 {
            for state in states.iter_mut() {
                if numbers.next().unwrap().is_multiple_of(3) {
                    *state = kinds[numbers.next().unwrap() % kinds.len()];
                }
            }
        }
        sections.push(Section::from_states(states, PLAINS));
    }
    let chunk = Chunk::from_sections(-32, sections);
    counts_and_heights_follow_the_blocks(&chunk);
    block_entities_follow_the_blocks(&chunk);
    assert!(!chunk.block_entities().is_empty());
    assert!(
        chunk
            .block_entities()
            .values()
            .all(|entity| entity.data.is_empty())
    );
    // By position, ordered by y, then z, then x.
    let positions: Vec<(i16, u8, u8)> = chunk
        .block_entities()
        .keys()
        .map(|at| (at.y, at.z, at.x))
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(chunk.generated(), 0);
    assert!(!chunk.with_failure());
    assert!(chunk.light().is_none());
    assert!(chunk.ticks().is_empty() && chunk.marks().is_empty());
}

#[test]
fn an_empty_chunk_has_no_heights_and_no_block_entities() {
    let chunk = Chunk::empty(overworld(), PLAINS);
    assert_eq!(kept(&chunk), [[0; 256]; 4]);
    assert!(chunk.block_entities().is_empty());
    // Cave air is air to every map.
    let mut chunk = chunk;
    chunk.set(4, 10, 4, blocks::CAVE_AIR);
    chunk.set(4, 11, 4, blocks::VOID_AIR);
    assert_eq!(kept(&chunk), [[0; 256]; 4]);
}

/// A column of stone to y 60, then water, leaves and a torch on top of one another,
/// taken away again from the top: each map rests on what counts for it.
#[test]
fn each_heightmap_rests_on_the_highest_block_that_counts_for_it() {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    let (x, z) = (5, 9);
    let column = z << 4 | x;
    let heights = |chunk: &Chunk| {
        let kept = kept(chunk);
        [
            kept[0][column],
            kept[1][column],
            kept[2][column],
            kept[3][column],
        ]
    };
    // A height is counted from the bottom of the dimension, which is at -64.
    let above = |y: i32| (y + 64 + 1) as u16;

    chunk.set(x, 60, z, blocks::STONE);
    assert_eq!(heights(&chunk), [above(60); 4]);
    chunk.set(x, 61, z, blocks::WATER);
    assert_eq!(
        heights(&chunk),
        [above(61), above(60), above(61), above(61)]
    );
    chunk.set(x, 70, z, blocks::OAK_LEAVES);
    assert_eq!(
        heights(&chunk),
        [above(70), above(70), above(70), above(61)]
    );
    chunk.set(x, 71, z, blocks::TORCH);
    assert_eq!(
        heights(&chunk),
        [above(71), above(70), above(70), above(61)]
    );
    // Nothing of another column moved.
    assert_eq!(kept(&chunk)[0].iter().filter(|h| **h != 0).count(), 1);

    chunk.set(x, 71, z, blocks::AIR);
    assert_eq!(
        heights(&chunk),
        [above(70), above(70), above(70), above(61)]
    );
    chunk.set(x, 70, z, blocks::CAVE_AIR);
    assert_eq!(
        heights(&chunk),
        [above(61), above(60), above(61), above(61)]
    );
    // A block that is not the one a height rests on leaves it.
    chunk.set(x, 30, z, blocks::STONE);
    chunk.set(x, 30, z, blocks::AIR);
    assert_eq!(
        heights(&chunk),
        [above(61), above(60), above(61), above(61)]
    );
    // The water becomes a waterlogged block, which counts as much; then stone goes
    // from under it.
    chunk.set(x, 61, z, waterlogged_slab());
    chunk.set(x, 60, z, blocks::AIR);
    assert_eq!(
        heights(&chunk),
        [
            above(61),
            if counts_for(waterlogged_slab())[1] {
                above(61)
            } else {
                0
            },
            above(61),
            above(61)
        ]
    );
    chunk.set(x, 61, z, blocks::AIR);
    assert_eq!(heights(&chunk), [0; 4]);
    counts_and_heights_follow_the_blocks(&chunk);
}

#[test]
fn heights_reach_the_top_and_the_bottom_of_the_dimension() {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    chunk.set(0, -64, 0, blocks::STONE);
    assert_eq!(chunk.heights().world_surface[0], 1);
    chunk.set(0, 319, 0, blocks::STONE);
    assert_eq!(chunk.heights().world_surface[0], 384);
    assert_eq!(chunk.heights().motion_blocking_no_leaves[0], 384);
    chunk.set(0, 319, 0, blocks::AIR);
    assert_eq!(chunk.heights().ocean_floor[0], 1);
    chunk.set(0, -64, 0, blocks::AIR);
    assert_eq!(chunk.heights().world_surface[0], 0);
    assert_eq!(chunk.set(0, 320, 0, blocks::STONE), None);
    assert_eq!(chunk.set(0, -65, 0, blocks::STONE), None);
    assert_eq!(chunk, Chunk::empty(overworld(), PLAINS));
}

/// A chunk as a generator leaves it: some ground, said to be as generated by version
/// 3 with a failure, with light, ticks and marks.
fn generated() -> Chunk {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    for x in 0..16 {
        chunk.set(x, 62, 8, blocks::STONE);
        chunk.set(x, 63, 8, blocks::GRASS_BLOCK);
    }
    chunk.set(2, 64, 8, blocks::GLASS);
    chunk.set_ticks(some_ticks());
    chunk.set_marks(some_marks());
    chunk.mark_generated(3, true);
    chunk.set_light(some_light(&chunk, 1));
    chunk
}

#[test]
fn a_generated_chunk_says_so_until_it_is_changed() {
    let chunk = generated();
    assert_eq!(chunk.generated(), 3);
    assert!(chunk.with_failure());
    assert_eq!(chunk.light(), Some(&some_light(&chunk, 1)));
}

#[test]
fn the_first_change_ends_as_generated_and_clears_the_failure_bit() {
    // Whether light sees the change or not, and whether it is one a client sees.
    for (x, y, new) in [
        (0, 62, blocks::IRON_ORE),
        (0, 63, blocks::DIRT),
        (0, 100, blocks::CAVE_AIR),
        (0, 100, blocks::TORCH),
        (0, 100, blocks::CHEST),
    ] {
        let mut chunk = generated();
        assert!(chunk.set(x, y, 8, new).is_some());
        assert_eq!(chunk.generated(), 0, "after {new:?}");
        assert!(!chunk.with_failure(), "after {new:?}");
    }
}

#[test]
fn a_change_that_is_taken_back_does_not_make_the_chunk_as_generated_again() {
    let mut chunk = generated();
    chunk.set(0, 62, 8, blocks::IRON_ORE);
    chunk.set(0, 62, 8, blocks::STONE);
    assert_eq!(chunk.generated(), 0);
    assert!(!chunk.with_failure());
    assert_ne!(chunk, generated());
    assert!(chunk.shows_the_same(&generated()));
}

#[test]
fn setting_the_state_that_is_there_keeps_the_chunk_as_generated_with_its_failure_bit() {
    let mut chunk = generated();
    assert_eq!(chunk.set(0, 62, 8, blocks::STONE), Some(blocks::STONE));
    assert_eq!(
        chunk.set(0, 63, 8, blocks::GRASS_BLOCK),
        Some(blocks::GRASS_BLOCK)
    );
    assert_eq!(chunk.set(0, 200, 8, blocks::AIR), Some(blocks::AIR));
    assert_eq!(chunk.set(2, 64, 8, blocks::GLASS), Some(blocks::GLASS));
    assert_eq!(chunk.generated(), 3);
    assert!(chunk.with_failure());
    assert!(chunk.light().is_some());
    assert_eq!(chunk, generated());
}

#[test]
fn a_set_outside_the_dimension_changes_nothing() {
    let mut chunk = generated();
    assert_eq!(chunk.set(0, 320, 8, blocks::TORCH), None);
    assert_eq!(chunk.set(0, -65, 8, blocks::TORCH), None);
    assert_eq!(chunk.set(0, i32::MAX, 8, blocks::TORCH), None);
    assert_eq!(chunk.set(0, i32::MIN, 8, blocks::TORCH), None);
    assert_eq!(chunk, generated());
}

#[test]
fn the_failure_bit_is_not_set_on_a_chunk_that_is_not_as_generated() {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    chunk.mark_generated(0, true);
    assert_eq!(chunk.generated(), 0);
    assert!(!chunk.with_failure());
    chunk.mark_generated(2, false);
    assert_eq!(chunk.generated(), 2);
    assert!(!chunk.with_failure());
    chunk.mark_generated(2, true);
    assert!(chunk.with_failure());
}

/// Whether a chunk that has `old` somewhere, and light, still has its light once
/// `new` is there. The light it has must be the very light it was given.
fn light_stays(old: BlockState, new: BlockState) -> bool {
    let mut chunk = generated();
    chunk.set(7, 80, 7, old);
    let light = some_light(&chunk, 1);
    chunk.set_light(light.clone());
    assert_eq!(chunk.set(7, 80, 7, new), Some(old));
    match chunk.light() {
        None => false,
        Some(kept) => {
            assert_eq!(kept, &light);
            true
        }
    }
}

#[test]
fn the_light_stays_when_stone_becomes_an_ore() {
    assert!(light_stays(blocks::STONE, blocks::IRON_ORE));
    assert!(light_stays(blocks::STONE, blocks::COAL_ORE));
}

#[test]
fn the_light_stays_when_an_ore_becomes_stone() {
    assert!(light_stays(blocks::IRON_ORE, blocks::STONE));
    assert!(light_stays(blocks::COAL_ORE, blocks::STONE));
}

#[test]
fn the_light_stays_when_grass_becomes_dirt() {
    assert!(light_stays(blocks::GRASS_BLOCK, blocks::DIRT));
    assert!(light_stays(blocks::DIRT, blocks::GRASS_BLOCK));
}

#[test]
fn the_light_stays_when_a_door_changes_its_state_and_keeps_its_faces() {
    assert!(light_stays(blocks::OAK_DOOR, powered_door()));
}

#[test]
fn the_light_stays_when_a_chest_is_turned() {
    assert!(!light_sees(blocks::CHEST, chest_facing_east()));
    assert!(light_stays(blocks::CHEST, chest_facing_east()));
}

#[test]
fn the_light_goes_at_a_torch() {
    assert!(!light_stays(blocks::AIR, blocks::TORCH));
    assert!(!light_stays(blocks::TORCH, blocks::AIR));
    assert!(!light_stays(blocks::STONE, blocks::GLOWSTONE));
    assert!(!light_stays(blocks::FURNACE, lit_furnace()));
}

#[test]
fn the_light_goes_when_water_comes_into_air() {
    assert!(!light_stays(blocks::AIR, blocks::WATER));
    assert!(!light_stays(blocks::AIR, flowing_water()));
    assert!(!light_stays(blocks::CAVE_AIR, blocks::WATER));
}

#[test]
fn the_light_goes_when_glass_becomes_stone() {
    assert!(!light_stays(blocks::GLASS, blocks::STONE));
    assert!(!light_stays(blocks::STONE, blocks::GLASS));
}

#[test]
fn the_light_goes_when_a_block_is_placed_in_air_or_broken() {
    assert!(!light_stays(blocks::AIR, blocks::STONE));
    assert!(!light_stays(blocks::STONE, blocks::AIR));
    assert!(!light_stays(blocks::AIR, blocks::OAK_LEAVES));
}

#[test]
fn light_that_went_does_not_come_back_when_the_change_is_taken_back() {
    let mut chunk = generated();
    chunk.set(7, 80, 7, blocks::TORCH);
    assert!(chunk.light().is_none());
    chunk.set(7, 80, 7, blocks::AIR);
    assert!(chunk.light().is_none());
    // And further changes that light does not see do not bring it either.
    chunk.set(0, 62, 8, blocks::IRON_ORE);
    assert!(chunk.light().is_none());
}

#[test]
fn a_chunk_is_changed_and_still_carries_its_light() {
    // "As generated" and "its light is valid" are two things.
    let mut chunk = generated();
    chunk.set(0, 62, 8, blocks::IRON_ORE);
    chunk.set(1, 63, 8, blocks::DIRT);
    assert_eq!(chunk.generated(), 0);
    assert_eq!(chunk.light(), Some(&some_light(&chunk, 1)));
}

#[test]
fn ticks_and_marks_are_untouched_by_changes() {
    let mut chunk = generated();
    let (ticks, marks) = (chunk.ticks().to_vec(), chunk.marks().to_vec());
    assert_eq!(ticks, some_ticks());
    assert_eq!(marks.len(), 3);
    // Among them the marked and the ticked positions themselves.
    for at in some_marks() {
        chunk.set(
            usize::from(at.x),
            i32::from(at.y),
            usize::from(at.z),
            blocks::WATER,
        );
    }
    for tick in some_ticks() {
        let at = tick.at;
        chunk.set(
            usize::from(at.x),
            i32::from(at.y),
            usize::from(at.z),
            blocks::TORCH,
        );
    }
    chunk.set(0, 62, 8, blocks::CHEST);
    chunk.set(0, 62, 8, blocks::AIR);
    assert_eq!(chunk.ticks(), ticks);
    assert_eq!(chunk.marks(), marks);
}

#[test]
fn marks_are_kept_ascending_and_each_once_and_ticks_in_the_order_given() {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    let (a, b, c) = (
        InChunk::new(5, -10, 0),
        InChunk::new(0, 3, 9),
        InChunk::new(1, 3, 9),
    );
    chunk.set_marks(vec![c, a, b, a, c]);
    assert_eq!(chunk.marks(), [a, b, c]);

    let mut ticks = some_ticks();
    ticks.reverse();
    chunk.set_ticks(ticks.clone());
    assert_eq!(chunk.ticks(), ticks);
}

#[test]
fn a_position_in_a_chunk_is_ordered_by_y_then_z_then_x() {
    let mut positions = vec![
        InChunk::new(0, 1, 0),
        InChunk::new(15, 0, 0),
        InChunk::new(0, 0, 1),
        InChunk::new(0, -1, 15),
        InChunk::new(1, 0, 0),
    ];
    positions.sort();
    assert_eq!(
        positions,
        [
            InChunk::new(0, -1, 15),
            InChunk::new(1, 0, 0),
            InChunk::new(15, 0, 0),
            InChunk::new(0, 0, 1),
            InChunk::new(0, 1, 0),
        ]
    );
}

#[test]
fn a_placed_chest_is_shown_and_a_broken_chest_leaves_nothing_behind() {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    let at = InChunk::new(3, 70, 4);
    chunk.set(3, 70, 4, blocks::CHEST);
    let chest = entity_type(blocks::CHEST).expect("a chest has one");
    assert_eq!(
        chunk.block_entities().get(&at),
        Some(&BlockEntity {
            kind: chest,
            data: Vec::new()
        })
    );
    assert_eq!(chunk.block_entities().len(), 1);

    chunk.set_block_entity_data(at, vec![1, 2, 3]).unwrap();
    chunk.set(3, 70, 4, blocks::AIR);
    assert!(chunk.block_entities().is_empty());
    // A chest there again knows nothing of the one before.
    chunk.set(3, 70, 4, blocks::CHEST);
    assert_eq!(chunk.block_entities()[&at].data, Vec::<u8>::new());
}

#[test]
fn a_block_entity_keeps_its_data_while_its_block_keeps_its_type() {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    let at = InChunk::new(3, 70, 4);
    chunk.set(3, 70, 4, blocks::CHEST);
    chunk.set_block_entity_data(at, vec![9; 50]).unwrap();
    // Turned: another state of the same block.
    chunk.set(3, 70, 4, chest_facing_east());
    assert_eq!(chunk.block_entities()[&at].data, vec![9; 50]);
    assert_eq!(
        Some(chunk.block_entities()[&at].kind),
        entity_type(blocks::CHEST)
    );
}

#[test]
fn a_block_of_another_block_entity_type_gets_an_entity_of_its_own_without_data() {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    let at = InChunk::new(3, 70, 4);
    chunk.set(3, 70, 4, blocks::CHEST);
    chunk.set_block_entity_data(at, vec![9; 50]).unwrap();
    for other in [blocks::FURNACE, blocks::OAK_SIGN, blocks::TRAPPED_CHEST] {
        chunk.set(3, 70, 4, other);
        assert_eq!(
            chunk.block_entities().get(&at),
            Some(&BlockEntity {
                kind: entity_type(other).expect("it has one"),
                data: Vec::new()
            }),
            "{other:?}"
        );
        assert_eq!(chunk.block_entities().len(), 1);
    }
    chunk.set(3, 70, 4, blocks::STONE);
    assert!(chunk.block_entities().is_empty());
}

#[test]
fn data_of_exactly_16_kib_is_taken_for_one_block_entity_and_one_byte_more_is_refused() {
    assert_eq!(MAX_BLOCK_ENTITY_DATA, SIXTEEN_KIB);
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    let at = InChunk::new(0, 0, 0);
    chunk.set(0, 0, 0, blocks::CHEST);

    assert_eq!(
        chunk.set_block_entity_data(at, vec![1; SIXTEEN_KIB]),
        Ok(())
    );
    assert_eq!(chunk.block_entities()[&at].data, vec![1; SIXTEEN_KIB]);

    assert_eq!(
        chunk.set_block_entity_data(at, vec![2; SIXTEEN_KIB + 1]),
        Err(DataRefused::TooMuchForOne)
    );
    // What was refused left what was there.
    assert_eq!(chunk.block_entities()[&at].data, vec![1; SIXTEEN_KIB]);

    // Less is taken, and none stands for an empty compound.
    assert_eq!(chunk.set_block_entity_data(at, vec![3; 15]), Ok(()));
    assert_eq!(chunk.set_block_entity_data(at, Vec::new()), Ok(()));
    assert!(chunk.block_entities()[&at].data.is_empty());
}

/// A chunk with 65 chests, 64 of which have 16 KiB of data each: 1 MiB together.
fn with_a_mebibyte_of_data() -> (Chunk, Vec<InChunk>) {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    let mut chests = Vec::new();
    for index in 0..65 {
        let (x, y, z) = (index % 16, 10 + (index / 16) as i32, 3);
        chunk.set(x, y, z, blocks::CHEST);
        chests.push(InChunk::new(x, y, z));
    }
    for (index, at) in chests.iter().take(64).enumerate() {
        let data = vec![index as u8; SIXTEEN_KIB];
        assert_eq!(
            chunk.set_block_entity_data(*at, data),
            Ok(()),
            "chest {index}"
        );
    }
    (chunk, chests)
}

fn data_of(chunk: &Chunk) -> usize {
    chunk
        .block_entities()
        .values()
        .map(|entity| entity.data.len())
        .sum()
}

#[test]
fn data_of_exactly_1_mib_is_taken_for_a_chunk_and_one_byte_more_is_refused() {
    assert_eq!(MAX_CHUNK_ENTITY_DATA, ONE_MIB);
    let (mut chunk, chests) = with_a_mebibyte_of_data();
    assert_eq!(data_of(&chunk), ONE_MIB);

    let before = chunk.clone();
    assert_eq!(
        chunk.set_block_entity_data(chests[64], vec![0xee]),
        Err(DataRefused::TooMuchForTheChunk)
    );
    assert_eq!(chunk, before, "what was refused changed nothing");
    assert!(chunk.block_entities()[&chests[64]].data.is_empty());
    assert_eq!(chunk.set_block_entity_data(chests[64], Vec::new()), Ok(()));
}

#[test]
fn a_block_entitys_new_data_is_counted_in_place_of_its_old() {
    let (mut chunk, chests) = with_a_mebibyte_of_data();
    // Other data of the same size for one that has its share already.
    assert_eq!(
        chunk.set_block_entity_data(chests[0], vec![0xaa; SIXTEEN_KIB]),
        Ok(())
    );
    assert_eq!(data_of(&chunk), ONE_MIB);
    // One byte less there makes room for one byte elsewhere, and for no more.
    assert_eq!(
        chunk.set_block_entity_data(chests[0], vec![0xaa; SIXTEEN_KIB - 1]),
        Ok(())
    );
    assert_eq!(
        chunk.set_block_entity_data(chests[64], vec![1, 2]),
        Err(DataRefused::TooMuchForTheChunk)
    );
    assert_eq!(chunk.set_block_entity_data(chests[64], vec![1]), Ok(()));
    assert_eq!(data_of(&chunk), ONE_MIB);
}

#[test]
fn data_that_went_with_its_block_makes_room_again() {
    let (mut chunk, chests) = with_a_mebibyte_of_data();
    let gone = chests[5];
    chunk.set(
        usize::from(gone.x),
        i32::from(gone.y),
        usize::from(gone.z),
        blocks::AIR,
    );
    assert_eq!(data_of(&chunk), ONE_MIB - SIXTEEN_KIB);
    assert_eq!(
        chunk.set_block_entity_data(chests[64], vec![4; SIXTEEN_KIB]),
        Ok(())
    );
    assert_eq!(data_of(&chunk), ONE_MIB);
}

#[test]
fn data_is_refused_where_no_block_entity_is() {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    chunk.set(1, 1, 1, blocks::STONE);
    for at in [
        InChunk::new(1, 1, 1),
        InChunk::new(2, 2, 2),
        InChunk::new(0, 5000, 0),
    ] {
        assert_eq!(
            chunk.set_block_entity_data(at, vec![1]),
            Err(DataRefused::NoBlockEntity)
        );
    }
    assert!(chunk.block_entities().is_empty());
}
