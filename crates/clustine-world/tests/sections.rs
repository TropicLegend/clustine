//! A section's blocks, counts, equality and biomes, tested from
//! `docs/adr/0022-what-a-chunk-carries.md` (sections 1 and 2 and the first of its
//! tests) through the crate's public interface alone.
//!
//! What a section looks like when it travels, and that a broken one is an error, is
//! tested where a serialiser is: `crates/clustine-rpc/tests/chunks_travel.rs`.

use clustine_data::{BLOCK_STATE_COUNT, BlockState, blocks};
use clustine_world::{BIOME_CELLS, BLOCKS_PER_SECTION, Biome, Section};

const PLAINS: Biome = Biome(0);

/// A sequence of numbers that looks random and is the same every time, so that a
/// failure can be had again.
fn numbers(mut seed: u64) -> impl Iterator<Item = usize> {
    std::iter::repeat_with(move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize
    })
}

fn waterlogged_slab() -> BlockState {
    BlockState::parse("minecraft:oak_slab[waterlogged=true]").expect("a slab can be waterlogged")
}

/// `count` distinct states of the game. The first are the ones the counts turn on:
/// the three airs, the two fluids and a block with water in it.
fn states(count: usize) -> Vec<BlockState> {
    let mut states = vec![
        blocks::AIR,
        blocks::STONE,
        blocks::WATER,
        blocks::CAVE_AIR,
        blocks::LAVA,
        waterlogged_slab(),
        blocks::VOID_AIR,
    ];
    // Spread over the whole table, so that ids of every size are among them.
    let mut id = 11;
    while states.len() < count {
        let state = BlockState(id as u16);
        if !states.contains(&state) {
            states.push(state);
        }
        id = (id + 7919) % BLOCK_STATE_COUNT;
    }
    states.truncate(count);
    states
}

/// The record's two counts, counted anew from the table: what is not air by `is_air`,
/// and what has a fluid.
fn recount(plain: &[BlockState]) -> (usize, usize) {
    (
        plain.iter().filter(|state| !state.props().is_air()).count(),
        plain
            .iter()
            .filter(|state| state.props().fluid().is_some())
            .count(),
    )
}

/// Fails unless `section` holds exactly `plain`, with the counts a recount gives.
fn holds(section: &Section, plain: &[BlockState; BLOCKS_PER_SECTION]) {
    assert!(section.states().eq(plain.iter().copied()));
    assert_eq!(section.states().count(), BLOCKS_PER_SECTION);
    let (non_air, fluids) = recount(plain);
    assert_eq!(usize::from(section.non_air_count()), non_air);
    assert_eq!(usize::from(section.fluid_count()), fluids);
}

fn position(at: usize) -> (usize, usize, usize) {
    (at & 15, at >> 8, at >> 4 & 15)
}

#[test]
fn the_states_the_tests_count_by_are_what_they_are_taken_for() {
    for air in [blocks::AIR, blocks::CAVE_AIR, blocks::VOID_AIR] {
        assert!(air.props().is_air());
        assert!(air.props().fluid().is_none());
    }
    for fluid in [blocks::WATER, blocks::LAVA, waterlogged_slab()] {
        assert!(!fluid.props().is_air());
        assert!(fluid.props().fluid().is_some());
    }
    assert!(!blocks::STONE.props().is_air());
    assert!(blocks::STONE.props().fluid().is_none());
    let many = states(3000);
    assert_eq!(many.len(), 3000);
    let mut sorted = many.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 3000, "the states are distinct");
}

/// The palette's widths end at 16, 32, 64, 128 and 256 states, so the numbers of
/// kinds lie on both sides of each, and far beyond the last.
#[test]
fn after_random_sets_a_section_holds_what_a_plain_array_holds() {
    for (seed, kinds) in [
        (1, 2),
        (2, 7),
        (3, 16),
        (4, 17),
        (5, 32),
        (6, 33),
        (7, 65),
        (8, 129),
        (9, 256),
        (10, 257),
        (11, 3000),
    ] {
        let kinds = states(kinds);
        let mut numbers = numbers(seed);
        let mut section = Section::filled(blocks::AIR, PLAINS);
        let mut plain = Box::new([blocks::AIR; BLOCKS_PER_SECTION]);
        for step in 0..8000 {
            let at = numbers.next().unwrap() % BLOCKS_PER_SECTION;
            // Half of the time one of the few the counts turn on, so that air and
            // fluids come and go however many kinds there are.
            let state = if numbers.next().unwrap() % 2 == 0 {
                kinds[numbers.next().unwrap() % kinds.len().min(7)]
            } else {
                kinds[numbers.next().unwrap() % kinds.len()]
            };
            let (x, y, z) = position(at);
            assert_eq!(section.set(x, y, z, state), plain[at], "what `set` returns");
            plain[at] = state;
            assert_eq!(section.get(x, y, z), state);
            if step % 251 == 0 {
                holds(&section, &plain);
            }
        }
        holds(&section, &plain);
        for (at, state) in plain.iter().enumerate() {
            let (x, y, z) = position(at);
            assert_eq!(section.get(x, y, z), *state);
        }
        // However it came to them, it equals the section made of these blocks at once.
        assert_eq!(section, Section::from_states(plain.clone(), PLAINS));
    }
}

#[test]
fn a_section_made_from_states_holds_them_in_the_order_of_y_then_z_then_x() {
    let kinds = states(300);
    let mut numbers = numbers(21);
    let mut plain = Box::new([blocks::AIR; BLOCKS_PER_SECTION]);
    for state in plain.iter_mut() {
        *state = kinds[numbers.next().unwrap() % kinds.len()];
    }
    let section = Section::from_states(plain.clone(), PLAINS);
    holds(&section, &plain);
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                assert_eq!(section.get(x, y, z), plain[y << 8 | z << 4 | x]);
            }
        }
    }
}

#[test]
fn a_section_of_cave_air_counts_no_blocks() {
    for air in [blocks::CAVE_AIR, blocks::VOID_AIR, blocks::AIR] {
        let filled = Section::filled(air, PLAINS);
        assert_eq!(filled.non_air_count(), 0);
        assert_eq!(filled.fluid_count(), 0);

        let made = Section::from_states(Box::new([air; BLOCKS_PER_SECTION]), PLAINS);
        assert_eq!(made.non_air_count(), 0);

        // And one that became so block by block.
        let mut set = Section::filled(blocks::STONE, PLAINS);
        assert_eq!(usize::from(set.non_air_count()), BLOCKS_PER_SECTION);
        for at in 0..BLOCKS_PER_SECTION {
            let (x, y, z) = position(at);
            set.set(x, y, z, air);
        }
        assert_eq!(set.non_air_count(), 0);
        assert_eq!(set, filled);
    }
}

#[test]
fn the_three_airs_mixed_count_no_blocks_and_one_stone_among_them_counts_one() {
    let mut section = Section::filled(blocks::AIR, PLAINS);
    section.set(1, 2, 3, blocks::CAVE_AIR);
    section.set(4, 5, 6, blocks::VOID_AIR);
    assert_eq!(section.non_air_count(), 0);
    section.set(7, 8, 9, blocks::STONE);
    assert_eq!(section.non_air_count(), 1);
    section.set(7, 8, 9, blocks::CAVE_AIR);
    assert_eq!(section.non_air_count(), 0);
}

#[test]
fn a_section_counts_water_lava_and_waterlogged_blocks_as_fluids() {
    let all = BLOCKS_PER_SECTION as u16;
    assert_eq!(Section::filled(blocks::WATER, PLAINS).fluid_count(), all);
    assert_eq!(Section::filled(blocks::WATER, PLAINS).non_air_count(), all);
    assert_eq!(Section::filled(blocks::STONE, PLAINS).fluid_count(), 0);

    let mut section = Section::filled(blocks::STONE, PLAINS);
    section.set(0, 0, 0, blocks::WATER);
    section.set(1, 0, 0, blocks::LAVA);
    section.set(2, 0, 0, waterlogged_slab());
    section.set(3, 0, 0, blocks::AIR);
    assert_eq!(section.fluid_count(), 3);
    assert_eq!(section.non_air_count(), all - 1);
    // The same state again counts nothing twice.
    section.set(0, 0, 0, blocks::WATER);
    assert_eq!(section.fluid_count(), 3);
    section.set(0, 0, 0, blocks::STONE);
    assert_eq!(section.fluid_count(), 2);
}

/// Puts `count` distinct states into the section and takes them out again, so that
/// it has been through whatever forms that many states need.
fn widened_and_emptied(count: usize) -> Section {
    let mut section = Section::filled(blocks::AIR, PLAINS);
    for (at, state) in states(count).into_iter().enumerate() {
        let (x, y, z) = position(at);
        section.set(x, y, z, state);
    }
    for at in 0..count {
        let (x, y, z) = position(at);
        section.set(x, y, z, blocks::AIR);
    }
    section
}

#[test]
fn a_section_that_went_to_eight_bits_and_back_to_two_states_equals_a_fresh_one() {
    // 17 need five bits, 200 need eight, and 300 an id for each position.
    for count in [17, 200, 300] {
        let mut worn = widened_and_emptied(count);
        worn.set(5, 5, 5, blocks::STONE);

        let mut fresh = Section::filled(blocks::AIR, PLAINS);
        fresh.set(5, 5, 5, blocks::STONE);
        assert_eq!(worn, fresh, "after {count} states");
        assert_eq!(fresh, worn);

        let mut plain = Box::new([blocks::AIR; BLOCKS_PER_SECTION]);
        plain[5 << 8 | 5 << 4 | 5] = blocks::STONE;
        assert_eq!(worn, Section::from_states(plain, PLAINS));
        assert_eq!(worn.non_air_count(), 1);
    }
}

#[test]
fn a_section_that_held_many_states_and_holds_one_again_equals_a_filled_one() {
    for count in [2, 17, 200, 300] {
        let worn = widened_and_emptied(count);
        assert_eq!(worn, Section::filled(blocks::AIR, PLAINS));
        assert_eq!(Section::filled(blocks::AIR, PLAINS), worn);
        assert_eq!(worn.non_air_count(), 0);
    }
}

#[test]
fn sections_that_differ_in_one_block_are_not_equal() {
    for count in [1, 17, 300] {
        let worn = widened_and_emptied(count);
        let mut other = worn.clone();
        assert_eq!(worn, other);
        other.set(15, 15, 15, blocks::STONE);
        assert_ne!(worn, other);
        // Cave air is air to the counts, and another block all the same.
        let mut cave = worn.clone();
        cave.set(15, 15, 15, blocks::CAVE_AIR);
        assert_ne!(worn, cave);
    }
    assert_ne!(
        Section::filled(blocks::AIR, PLAINS),
        Section::filled(blocks::CAVE_AIR, PLAINS)
    );
}

#[test]
fn setting_the_state_that_is_there_changes_nothing() {
    let mut section = Section::filled(blocks::STONE, PLAINS);
    assert_eq!(section.set(3, 3, 3, blocks::STONE), blocks::STONE);
    assert_eq!(section, Section::filled(blocks::STONE, PLAINS));
    assert_eq!(usize::from(section.non_air_count()), BLOCKS_PER_SECTION);
}

/// The cells a block lies in, by the record: 64 values indexed by `y << 4 | z << 2 | x`
/// in cells of four blocks.
fn cell_of(x: usize, y: usize, z: usize) -> usize {
    (y / 4) << 4 | (z / 4) << 2 | (x / 4)
}

#[test]
fn a_section_has_a_biome_for_each_of_its_64_cells_of_four_blocks() {
    assert_eq!(BIOME_CELLS, 64);
    let mut cells = Box::new([PLAINS; BIOME_CELLS]);
    for (cell, biome) in cells.iter_mut().enumerate() {
        *biome = Biome(100 + cell as u16);
    }
    let mut section = Section::filled(blocks::STONE, PLAINS);
    section.set_biomes(cells.clone());

    assert!(section.biomes().eq(cells.iter().copied()));
    assert_eq!(section.biomes().count(), BIOME_CELLS);
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                assert_eq!(section.biome_at(x, y, z), cells[cell_of(x, y, z)]);
            }
        }
    }
    // The one biome of those who still take a section to have one is the first cell's.
    assert_eq!(section.biome(), cells[0]);
    assert_eq!(section.uniform_biome(), None);
}

#[test]
fn a_section_of_one_biome_has_it_in_every_cell() {
    let section = Section::filled(blocks::STONE, Biome(7));
    assert_eq!(section.biomes().count(), BIOME_CELLS);
    assert!(section.biomes().all(|biome| biome == Biome(7)));
    assert_eq!(section.biome_at(15, 15, 15), Biome(7));
    assert_eq!(section.biome(), Biome(7));
    assert_eq!(section.uniform_biome(), Some(Biome(7)));

    let made = Section::from_states(Box::new([blocks::STONE; BLOCKS_PER_SECTION]), Biome(7));
    assert!(made.biomes().all(|biome| biome == Biome(7)));
}

#[test]
fn sections_are_equal_if_their_biomes_are_however_they_were_given() {
    let one = Section::filled(blocks::STONE, Biome(7));
    let mut cells = Section::filled(blocks::STONE, PLAINS);
    cells.set_biomes(Box::new([Biome(7); BIOME_CELLS]));
    assert_eq!(one, cells);
    assert_eq!(cells, one);

    // Sixty-four cells that were two biomes and are one again.
    let mut mixed = Box::new([Biome(7); BIOME_CELLS]);
    mixed[63] = Biome(8);
    let mut back = Section::filled(blocks::STONE, PLAINS);
    back.set_biomes(mixed.clone());
    assert_ne!(one, back);
    back.set_biomes(Box::new([Biome(7); BIOME_CELLS]));
    assert_eq!(one, back);

    assert_ne!(one, Section::filled(blocks::STONE, Biome(8)));
}

#[test]
fn sections_that_differ_in_the_biome_of_one_cell_are_not_equal() {
    for cell in [0, 1, 37, 63] {
        let mut cells = Box::new([PLAINS; BIOME_CELLS]);
        cells[cell] = Biome(3);
        let mut section = Section::filled(blocks::STONE, PLAINS);
        section.set_biomes(cells);
        assert_ne!(section, Section::filled(blocks::STONE, PLAINS));
        assert_eq!(
            section.biomes().filter(|biome| *biome == Biome(3)).count(),
            1
        );
    }
}

#[test]
fn changing_blocks_leaves_the_biomes_as_they_were() {
    let mut cells = Box::new([PLAINS; BIOME_CELLS]);
    for (cell, biome) in cells.iter_mut().enumerate() {
        *biome = Biome(cell as u16 % 3);
    }
    let mut section = Section::filled(blocks::AIR, PLAINS);
    section.set_biomes(cells.clone());
    // Through every form of the blocks.
    for (at, state) in states(300).into_iter().enumerate() {
        let (x, y, z) = position(at);
        section.set(x, y, z, state);
    }
    assert!(section.biomes().eq(cells.iter().copied()));
}
