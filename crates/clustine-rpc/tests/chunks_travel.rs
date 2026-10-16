//! How a section and a chunk travel between services, tested from
//! `docs/adr/0022-what-a-chunk-carries.md` (sections 2 and 4 and the first of its
//! tests): the serialised form comes back equal, a 17th and a 257th state move a
//! section's form, and a broken one is an error, not a panic.
//!
//! These are here and not in `clustine-world` because that crate has no serialiser to
//! test with; this one writes every message with postcard. The types `…OnWire` say
//! again, from the record, what travels: "per section the form, the palette and the
//! words as bytes; the biomes; the block entities; the version and the failure bit;
//! the light; ticks and marks", and since ADR-0024 the seam entries, and not the counts
//! and not the heightmaps. A section
//! or a chunk is read into them to see its form, and broken ones are written from
//! them.

use clustine_data::{BLOCK_INFO, BLOCK_STATE_COUNT, BlockState, blocks};
use clustine_world::{
    BIOME_CELLS, BLOCKS_PER_SECTION, Biome, BlockEntity, Chunk, InChunk, LIGHT_ARRAY_LENGTH, Light,
    ScheduledTick, Seam, Section, SectionLight, Ticked,
};
use serde::{Deserialize, Serialize};

const PLAINS: Biome = Biome(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SectionOnWire {
    blocks: BlocksOnWire,
    biomes: BiomesOnWire,
}

/// The three forms of the record's section 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum BlocksOnWire {
    Uniform(BlockState),
    Indexed {
        bits: u8,
        palette: Vec<BlockState>,
        words: Vec<u8>,
    },
    Direct(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum BiomesOnWire {
    One(Biome),
    Cells(Vec<Biome>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ChunkOnWire {
    min_y: i32,
    sections: Vec<SectionOnWire>,
    block_entities: Vec<(InChunk, BlockEntity)>,
    generated: u16,
    with_failure: bool,
    light: Option<Light>,
    ticks: Vec<ScheduledTick>,
    marks: Vec<InChunk>,
    seams: Vec<Seam>,
}

fn bytes_of<T: Serialize>(value: &T) -> Vec<u8> {
    postcard::to_stdvec(value).expect("it can be serialised")
}

/// What `section` travels as. Fails unless the bytes are those of the record's shape
/// and nothing else.
fn on_wire(section: &Section) -> SectionOnWire {
    let bytes = bytes_of(section);
    let read: SectionOnWire = postcard::from_bytes(&bytes).expect("a section as the record has it");
    assert_eq!(bytes_of(&read), bytes);
    read
}

fn chunk_on_wire(chunk: &Chunk) -> ChunkOnWire {
    let bytes = bytes_of(chunk);
    let read: ChunkOnWire = postcard::from_bytes(&bytes).expect("a chunk as the record has it");
    assert_eq!(bytes_of(&read), bytes);
    read
}

fn section_from(wire: &SectionOnWire) -> Result<Section, postcard::Error> {
    postcard::from_bytes(&bytes_of(wire))
}

fn chunk_from(wire: &ChunkOnWire) -> Result<Chunk, postcard::Error> {
    postcard::from_bytes(&bytes_of(wire))
}

/// A sequence of numbers that looks random and is the same every time.
fn numbers(mut seed: u64) -> impl Iterator<Item = usize> {
    std::iter::repeat_with(move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize
    })
}

/// `count` distinct states of the game, air first, with water and cave air among the
/// first few.
fn states(count: usize) -> Vec<BlockState> {
    let mut states = vec![blocks::AIR, blocks::STONE, blocks::WATER, blocks::CAVE_AIR];
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

fn position(at: usize) -> (usize, usize, usize) {
    (at & 15, at >> 8, at >> 4 & 15)
}

/// A section of air into which the first `count` of [`states`] were put one after
/// the other, at the positions 0, 1, 2 and so on (air being there from the start).
fn with_states(count: usize) -> Section {
    let mut section = Section::filled(blocks::AIR, PLAINS);
    for (at, state) in states(count).into_iter().enumerate() {
        let (x, y, z) = position(at);
        section.set(x, y, z, state);
    }
    section
}

/// Fails unless the two hold the same, with the counts a recount gives: the counts do
/// not travel, the reader makes them.
fn came_back(read: &Section, section: &Section) {
    assert_eq!(read, section);
    assert!(read.states().eq(section.states()));
    assert!(read.biomes().eq(section.biomes()));
    let non_air = section.states().filter(|s| !s.props().is_air()).count();
    let fluids = section
        .states()
        .filter(|s| s.props().fluid().is_some())
        .count();
    assert_eq!(usize::from(read.non_air_count()), non_air);
    assert_eq!(usize::from(read.fluid_count()), fluids);
}

#[test]
fn the_serialised_form_of_a_section_comes_back_equal_through_every_form() {
    for (seed, kinds) in [
        (1, 1),
        (2, 2),
        (3, 16),
        (4, 17),
        (5, 33),
        (6, 65),
        (7, 129),
        (8, 256),
        (9, 257),
        (10, 3000),
    ] {
        let kinds = states(kinds);
        let mut numbers = numbers(seed);
        let mut section = Section::filled(blocks::AIR, PLAINS);
        for step in 0..6000 {
            let (x, y, z) = position(numbers.next().unwrap() % BLOCKS_PER_SECTION);
            section.set(x, y, z, kinds[numbers.next().unwrap() % kinds.len()]);
            if step % 1499 == 0 {
                let read: Section = postcard::from_bytes(&bytes_of(&section)).unwrap();
                came_back(&read, &section);
            }
        }
        let mut cells = Box::new([PLAINS; BIOME_CELLS]);
        for cell in cells.iter_mut() {
            *cell = Biome((numbers.next().unwrap() % 3) as u16);
        }
        section.set_biomes(cells);
        let read: Section = postcard::from_bytes(&bytes_of(&section)).unwrap();
        came_back(&read, &section);
        // What came back travels as what was sent.
        assert_eq!(bytes_of(&read), bytes_of(&section));
    }
}

#[test]
fn a_uniform_section_travels_as_one_state_and_one_biome() {
    let section = Section::filled(blocks::STONE, Biome(3));
    assert_eq!(
        on_wire(&section),
        SectionOnWire {
            blocks: BlocksOnWire::Uniform(blocks::STONE),
            biomes: BiomesOnWire::One(Biome(3)),
        }
    );
    assert!(bytes_of(&section).len() <= 8);
    let read: Section = postcard::from_bytes(&bytes_of(&section)).unwrap();
    came_back(&read, &section);
}

#[test]
fn a_section_of_several_biomes_travels_with_its_64_cells_in_order() {
    let mut cells = Box::new([PLAINS; BIOME_CELLS]);
    for (cell, biome) in cells.iter_mut().enumerate() {
        *biome = Biome(cell as u16 + 1);
    }
    let mut section = Section::filled(blocks::STONE, PLAINS);
    section.set_biomes(cells.clone());
    assert_eq!(
        on_wire(&section).biomes,
        BiomesOnWire::Cells(cells.to_vec())
    );
    let read: Section = postcard::from_bytes(&bytes_of(&section)).unwrap();
    came_back(&read, &section);
}

/// The record's table of forms: bits of an index and bytes of indices for a number
/// of states; `None` for the direct form.
fn form_for(count: usize) -> (Option<u8>, usize) {
    match count {
        2..=16 => (Some(4), 2048),
        17..=32 => (Some(5), 2736),
        33..=64 => (Some(6), 3280),
        65..=128 => (Some(7), 3648),
        129..=256 => (Some(8), 4096),
        _ => (None, 8192),
    }
}

#[test]
fn a_17th_and_a_257th_state_move_the_form_of_a_section() {
    for count in [
        2, 3, 15, 16, 17, 18, 32, 33, 64, 65, 128, 129, 255, 256, 257, 258, 400,
    ] {
        let section = with_states(count);
        let (bits, index_bytes) = form_for(count);
        let wire = on_wire(&section);
        match (&wire.blocks, bits) {
            (
                BlocksOnWire::Indexed {
                    bits,
                    palette,
                    words,
                },
                Some(expected),
            ) => {
                assert_eq!(*bits, expected, "{count} states");
                assert_eq!(words.len(), index_bytes, "{count} states");
                // "A palette … in the order they were first put there."
                assert_eq!(palette, &states(count), "{count} states");
            }
            (BlocksOnWire::Direct(words), None) => {
                assert_eq!(words.len(), index_bytes, "{count} states");
            }
            (other, _) => panic!("{count} states travel as {other:?}"),
        }
        // The size shows the form by itself: the indices, and little beside them.
        let size = bytes_of(&section).len();
        let beside = if bits.is_some() { 3 * count + 16 } else { 16 };
        assert!(
            (index_bytes..=index_bytes + beside).contains(&size),
            "{count} states are {size} bytes"
        );
        let read: Section = postcard::from_bytes(&bytes_of(&section)).unwrap();
        came_back(&read, &section);
    }
}

/// The value at `index` of values of `bits` bits packed "into 64-bit words, lowest
/// bits first, none across two words". (That the bytes of a word travel lowest first
/// the record does not say; it is taken so here.)
fn unpacked(words: &[u8], bits: u8, index: usize) -> u16 {
    let per_word = 64 / usize::from(bits);
    let word = &words[index / per_word * 8..][..8];
    let word = u64::from_le_bytes(word.try_into().expect("eight bytes"));
    (word >> (index % per_word * usize::from(bits)) & ((1 << bits) - 1)) as u16
}

#[test]
fn the_indices_of_a_section_are_packed_lowest_bits_first_and_none_across_two_words() {
    for count in [2, 16, 17, 33, 65, 129, 256, 257, 1000] {
        let kinds = states(count);
        let mut numbers = numbers(count as u64);
        let mut section = with_states(count);
        for _ in 0..5000 {
            let (x, y, z) = position(numbers.next().unwrap() % BLOCKS_PER_SECTION);
            section.set(x, y, z, kinds[numbers.next().unwrap() % kinds.len()]);
        }
        let plain: Vec<BlockState> = section.states().collect();
        match on_wire(&section).blocks {
            BlocksOnWire::Indexed {
                bits,
                palette,
                words,
            } => {
                for (at, state) in plain.iter().enumerate() {
                    let index = usize::from(unpacked(&words, bits, at));
                    assert_eq!(palette[index], *state, "{count} states, position {at}");
                }
            }
            // "4,096 state ids of 16 bits, four to a word."
            BlocksOnWire::Direct(words) => {
                for (at, state) in plain.iter().enumerate() {
                    assert_eq!(unpacked(&words, 16, at), state.0, "position {at}");
                }
            }
            BlocksOnWire::Uniform(_) => panic!("{count} states are not one"),
        }
    }
}

#[test]
fn a_section_never_moves_back_by_itself_and_its_palette_may_hold_states_no_longer_used() {
    let mut section = with_states(17);
    for at in 0..17 {
        let (x, y, z) = position(at);
        section.set(x, y, z, blocks::AIR);
    }
    section.set(0, 0, 0, blocks::STONE);
    let BlocksOnWire::Indexed { bits, palette, .. } = on_wire(&section).blocks else {
        panic!("it stays indexed");
    };
    assert_eq!(bits, 5);
    assert_eq!(palette, states(17));

    // It is the section of these blocks all the same, here and where it arrives.
    let mut fresh = Section::filled(blocks::AIR, PLAINS);
    fresh.set(0, 0, 0, blocks::STONE);
    assert_eq!(section, fresh);
    let read: Section = postcard::from_bytes(&bytes_of(&section)).unwrap();
    came_back(&read, &fresh);

    // Beyond the palette likewise.
    let mut direct = with_states(257);
    for at in 0..257 {
        let (x, y, z) = position(at);
        direct.set(x, y, z, blocks::AIR);
    }
    assert!(matches!(on_wire(&direct).blocks, BlocksOnWire::Direct(_)));
    assert_eq!(direct, Section::filled(blocks::AIR, PLAINS));
    let read: Section = postcard::from_bytes(&bytes_of(&direct)).unwrap();
    came_back(&read, &Section::filled(blocks::AIR, PLAINS));
}

/// A section of three states at four bits, as it travels.
fn three_states() -> (BlocksOnWire, Vec<BlockState>, Vec<u8>) {
    let wire = on_wire(&with_states(3));
    let BlocksOnWire::Indexed {
        bits: 4,
        palette,
        words,
    } = wire.blocks.clone()
    else {
        panic!("three states are indexed by four bits");
    };
    (wire.blocks, palette, words)
}

fn section_of(blocks: BlocksOnWire) -> SectionOnWire {
    SectionOnWire {
        blocks,
        biomes: BiomesOnWire::One(PLAINS),
    }
}

#[test]
fn a_section_as_it_travels_written_by_hand_is_read() {
    // So that the refusals below are of what is broken and not of the hand.
    let (blocks, _, _) = three_states();
    let read = section_from(&section_of(blocks)).unwrap();
    assert_eq!(read, with_states(3));
}

#[test]
fn a_section_with_an_index_beyond_its_palette_is_an_error_and_not_a_panic() {
    let (_, palette, words) = three_states();
    // Every position of the section in turn would be too much; the first, one in
    // the middle of a word, and the last.
    for at in [0, 7, 4095] {
        for beyond in [3, 15] {
            let mut words = words.clone();
            words[at / 2] |= beyond << (at % 2 * 4);
            let broken = section_of(BlocksOnWire::Indexed {
                bits: 4,
                palette: palette.clone(),
                words,
            });
            assert!(section_from(&broken).is_err(), "index {beyond} at {at}");
        }
    }
    // The last index within the palette is none of that.
    let mut words = words.clone();
    words[2047] = 0x20;
    let whole = section_of(BlocksOnWire::Indexed {
        bits: 4,
        palette: palette.clone(),
        words,
    });
    assert_eq!(section_from(&whole).unwrap().get(15, 15, 15), palette[2]);
}

#[test]
fn a_section_with_an_index_beyond_its_palette_at_every_width_is_an_error() {
    for (count, bits) in [(17, 5), (33, 6), (65, 7), (129, 8)] {
        let BlocksOnWire::Indexed {
            bits: found,
            palette,
            mut words,
        } = on_wire(&with_states(count)).blocks
        else {
            panic!("{count} states are indexed");
        };
        assert_eq!(found, bits);
        // All bits of the first index: the highest index of the width.
        words[0] = 0xff;
        words[1] = 0xff;
        let broken = section_of(BlocksOnWire::Indexed {
            bits,
            palette,
            words,
        });
        assert!(section_from(&broken).is_err(), "at {bits} bits");
    }
}

#[test]
fn a_section_with_too_few_or_too_many_words_is_an_error_and_not_a_panic() {
    let (_, palette, words) = three_states();
    for length in [0, 1, 8, 2040, 2047, 2049, 2056, 2736, 4096, 8192] {
        let mut words = words.clone();
        words.resize(length, 0);
        let broken = section_of(BlocksOnWire::Indexed {
            bits: 4,
            palette: palette.clone(),
            words,
        });
        assert!(
            section_from(&broken).is_err(),
            "{length} bytes at four bits"
        );
    }
    for length in [0, 8, 2048, 8184, 8191, 8193, 8200] {
        let broken = section_of(BlocksOnWire::Direct(vec![0; length]));
        assert!(section_from(&broken).is_err(), "{length} bytes of ids");
    }
    assert!(section_from(&section_of(BlocksOnWire::Direct(vec![0; 8192]))).is_ok());
}

#[test]
fn a_section_indexed_by_a_width_that_is_no_form_is_an_error_and_not_a_panic() {
    let (_, palette, _) = three_states();
    for bits in [0, 1, 2, 3, 9, 12, 16, 32, 63, 64, 65, 128, 255] {
        // With as many words as that width would need, and with as many as four bits.
        let per_word = 64usize.checked_div(usize::from(bits)).unwrap_or(0);
        let fitting = BLOCKS_PER_SECTION.checked_div(per_word).unwrap_or(0) * 8;
        for length in [fitting, 2048, 0] {
            let broken = section_of(BlocksOnWire::Indexed {
                bits,
                palette: palette.clone(),
                words: vec![0; length],
            });
            assert!(
                section_from(&broken).is_err(),
                "{bits} bits, {length} bytes"
            );
        }
    }
}

#[test]
fn a_section_whose_palette_is_empty_or_larger_than_its_width_is_an_error() {
    let empty = section_of(BlocksOnWire::Indexed {
        bits: 4,
        palette: Vec::new(),
        words: vec![0; 2048],
    });
    assert!(section_from(&empty).is_err());
    for (count, bits, bytes) in [(17, 4, 2048), (33, 5, 2736), (257, 8, 4096)] {
        let large = section_of(BlocksOnWire::Indexed {
            bits,
            palette: states(count),
            words: vec![0; bytes],
        });
        assert!(
            section_from(&large).is_err(),
            "{count} states at {bits} bits"
        );
    }
}

#[test]
fn a_section_with_another_number_of_biomes_than_64_is_an_error() {
    for count in [0, 1, 63, 65, 4096] {
        let broken = SectionOnWire {
            blocks: BlocksOnWire::Uniform(blocks::STONE),
            biomes: BiomesOnWire::Cells(vec![PLAINS; count]),
        };
        assert!(section_from(&broken).is_err(), "{count} biomes");
    }
    let whole = SectionOnWire {
        blocks: BlocksOnWire::Uniform(blocks::STONE),
        biomes: BiomesOnWire::Cells(vec![Biome(2); 64]),
    };
    assert_eq!(
        section_from(&whole).unwrap(),
        Section::filled(blocks::STONE, Biome(2))
    );
}

#[test]
fn a_section_that_is_cut_short_is_an_error_and_not_a_panic() {
    for count in [1, 3, 17, 300] {
        let bytes = bytes_of(&with_states(count));
        for length in [0, 1, 2, 5, bytes.len() / 2, bytes.len() - 1] {
            let length = length.min(bytes.len() - 1);
            assert!(
                postcard::from_bytes::<Section>(&bytes[..length]).is_err(),
                "{count} states, {length} bytes of {}",
                bytes.len()
            );
        }
    }
}

/// Whatever `section` holds, it can be asked without a panic and its counts are those
/// of its blocks. A state that is none of the game's may be among them here, so the
/// recount is by the two questions that take any number.
fn is_sound(section: &Section) {
    let states: Vec<BlockState> = section.states().collect();
    assert_eq!(states.len(), BLOCKS_PER_SECTION);
    let non_air = states.iter().filter(|state| !state.is_air()).count();
    let fluids = states.iter().filter(|state| state.has_fluid()).count();
    assert_eq!(usize::from(section.non_air_count()), non_air);
    assert_eq!(usize::from(section.fluid_count()), fluids);
    assert_eq!(section.biomes().count(), BIOME_CELLS);
    assert_eq!(section.get(15, 15, 15), states[4095]);
}

#[test]
fn a_section_with_bytes_changed_at_random_is_an_error_or_a_sound_section_never_a_panic() {
    let mut numbers = numbers(77);
    let mut refused = 0;
    for count in [1, 3, 16, 17, 100, 256, 300] {
        let bytes = bytes_of(&with_states(count));
        for _ in 0..300 {
            let mut bytes = bytes.clone();
            for _ in 0..1 + numbers.next().unwrap() % 3 {
                // Mostly in the head, where the form, the widths and the lengths are.
                let at = if numbers.next().unwrap().is_multiple_of(2) {
                    numbers.next().unwrap() % bytes.len().min(24)
                } else {
                    numbers.next().unwrap() % bytes.len()
                };
                bytes[at] = numbers.next().unwrap() as u8;
            }
            match postcard::from_bytes::<Section>(&bytes) {
                Ok(section) => is_sound(&section),
                Err(_) => refused += 1,
            }
        }
    }
    assert!(refused > 100, "only {refused} were refused");
}

#[test]
fn a_section_with_a_state_that_is_none_of_the_games_is_not_a_panic() {
    // The record names what the reader of a file does with such an id (it refuses
    // it) and not what the receiver of a message does; either way it must not fall.
    let unknown = BlockState(u16::MAX);
    assert!(u32::from(unknown.0) >= BLOCK_STATE_COUNT);
    let mut ids = vec![0u8; 8192];
    ids[0] = 0xff;
    ids[1] = 0xff;
    let cases = [
        section_of(BlocksOnWire::Uniform(unknown)),
        section_of(BlocksOnWire::Indexed {
            bits: 4,
            palette: vec![unknown, blocks::STONE],
            words: vec![0; 2048],
        }),
        section_of(BlocksOnWire::Direct(ids)),
    ];
    for case in cases {
        if let Ok(section) = section_from(&case) {
            is_sound(&section);
            // And a chunk of it likewise, with its heights and block entities.
            let chunk = ChunkOnWire {
                min_y: 0,
                sections: vec![case.clone()],
                block_entities: Vec::new(),
                generated: 0,
                with_failure: false,
                light: None,
                ticks: Vec::new(),
                marks: Vec::new(),
                seams: Vec::new(),
            };
            if let Ok(chunk) = chunk_from(&chunk) {
                assert_eq!(chunk.sections().len(), 1);
            }
        }
    }
}

fn entity_type(state: BlockState) -> Option<u8> {
    BLOCK_INFO[state.block_id().expect("a state of the game")].block_entity_type
}

fn some_light(sections: usize) -> Light {
    let entries = sections + 2;
    let mut levels = Box::new([0; LIGHT_ARRAY_LENGTH]);
    levels[100] = 0x7c;
    let mut sky = vec![SectionLight::Absent; entries];
    sky[0] = SectionLight::Dark;
    sky[1] = SectionLight::Levels(levels.clone());
    sky[2] = SectionLight::Full;
    let mut block = vec![SectionLight::Dark; entries];
    levels[5] = 1;
    block[2] = SectionLight::Levels(levels);
    Light {
        made_by: 1,
        beside: 3,
        sky,
        block,
    }
}

/// A chunk of four sections with one of everything: blocks in several forms, two
/// biomes, block entities with and without data, the version with the failure bit,
/// light of all four cases, ticks and marks.
fn full() -> Chunk {
    let mut cells = Box::new([PLAINS; BIOME_CELLS]);
    cells[9] = Biome(4);
    let mut mixed = with_states(40);
    mixed.set_biomes(cells);
    let sections = vec![
        Section::filled(blocks::STONE, PLAINS),
        mixed,
        with_states(300),
        Section::filled(blocks::AIR, PLAINS),
    ];
    let mut chunk = Chunk::from_sections(-16, sections);
    chunk.set(1, 40, 1, blocks::CHEST);
    chunk.set(2, 40, 1, blocks::OAK_SIGN);
    chunk.set(3, 40, 1, blocks::WATER);
    chunk.set(4, 41, 1, blocks::OAK_LEAVES);
    chunk
        .set_block_entity_data(InChunk::new(1, 40, 1), vec![10, 1, 2, 3, 0])
        .unwrap();
    chunk.set_ticks(vec![
        ScheduledTick {
            at: InChunk::new(3, 40, 1),
            of: Ticked::Fluid(1),
            delay: 5,
            priority: 0,
        },
        ScheduledTick {
            at: InChunk::new(0, -16, 0),
            of: Ticked::Block(300),
            delay: -1,
            priority: -3,
        },
    ]);
    chunk.set_marks(vec![InChunk::new(3, 40, 1), InChunk::new(15, -16, 15)]);
    chunk.mark_generated(7, true);
    chunk.set_light(some_light(4));
    chunk
}

#[test]
fn a_chunk_comes_back_equal_with_everything_it_carries() {
    let chunk = full();
    let read: Chunk = postcard::from_bytes(&bytes_of(&chunk)).unwrap();
    assert_eq!(read, chunk);
    assert!(read.shows_the_same(&chunk));
    assert_eq!(read.generated(), 7);
    assert!(read.with_failure());
    assert_eq!(read.light(), Some(&some_light(4)));
    assert_eq!(read.ticks(), chunk.ticks());
    assert_eq!(read.marks(), chunk.marks());
    assert_eq!(read.block_entities(), chunk.block_entities());
    assert_eq!(read.block_entities().len(), chunk.block_entities().len());
    // The heights and the counts do not travel: the reader makes them.
    assert_eq!(read.heights(), chunk.heights());
    for (read, sent) in read.sections().iter().zip(chunk.sections()) {
        assert_eq!(read.non_air_count(), sent.non_air_count());
        assert_eq!(read.fluid_count(), sent.fluid_count());
    }
    assert_eq!(bytes_of(&read), bytes_of(&chunk));
}

#[test]
fn a_chunk_travels_without_its_counts_and_heightmaps() {
    let chunk = full();
    let wire = chunk_on_wire(&chunk);
    assert_eq!(wire.min_y, -16);
    assert_eq!(wire.sections.len(), 4);
    assert_eq!(wire.generated, 7);
    assert!(wire.with_failure);
    assert_eq!(wire.light, Some(some_light(4)));
    assert_eq!(wire.ticks, chunk.ticks());
    assert_eq!(wire.marks, chunk.marks());
    let entities: Vec<(InChunk, BlockEntity)> = chunk
        .block_entities()
        .iter()
        .map(|(at, entity)| (*at, entity.clone()))
        .collect();
    assert_eq!(wire.block_entities, entities);
    // The four heightmaps are 2 KB in memory; a chunk of air has no room for them.
    let empty = Chunk::from_sections(0, vec![Section::filled(blocks::AIR, PLAINS); 24]);
    assert!(bytes_of(&empty).len() < 200);
}

#[test]
fn a_chunk_for_viewers_travels_without_ticks_and_marks_and_comes_back_equal() {
    let shown = full().for_viewers();
    let wire = chunk_on_wire(&shown);
    assert!(wire.ticks.is_empty() && wire.marks.is_empty());
    let read: Chunk = postcard::from_bytes(&bytes_of(&shown)).unwrap();
    assert_eq!(read, shown);
    assert!(read.shows_the_same(&full()));
    assert_ne!(read, full());
}

#[test]
fn a_chunk_that_arrives_without_a_block_entity_its_blocks_ask_for_is_given_one() {
    let chunk = full();
    let mut wire = chunk_on_wire(&chunk);
    let chest = InChunk::new(1, 40, 1);
    wire.block_entities.retain(|(at, _)| *at != chest);
    let read = chunk_from(&wire).unwrap();
    assert_eq!(
        read.block_entities().get(&chest),
        Some(&BlockEntity {
            kind: entity_type(blocks::CHEST).unwrap(),
            data: Vec::new()
        })
    );
    assert_eq!(read.block_entities().len(), chunk.block_entities().len());
    assert_eq!(
        read.block_entities()
            .get(&InChunk::new(2, 40, 1))
            .map(|e| e.kind),
        entity_type(blocks::OAK_SIGN)
    );

    // With none at all.
    wire.block_entities.clear();
    let read = chunk_from(&wire).unwrap();
    assert_eq!(read.block_entities().len(), chunk.block_entities().len());
    assert!(read.block_entities().values().all(|e| e.data.is_empty()));
}

#[test]
fn a_chunk_that_arrives_with_a_block_entity_its_blocks_do_not_ask_for_drops_it() {
    let chunk = full();
    let mut wire = chunk_on_wire(&chunk);
    let chest = entity_type(blocks::CHEST).unwrap();
    // On water, in air, and outside the chunk altogether.
    for at in [
        InChunk::new(3, 40, 1),
        InChunk::new(9, 45, 9),
        InChunk::new(0, 3000, 0),
        InChunk::new(0, -17, 0),
    ] {
        wire.block_entities.push((
            at,
            BlockEntity {
                kind: chest,
                data: vec![1],
            },
        ));
    }
    let read = chunk_from(&wire).unwrap();
    assert_eq!(read.block_entities(), chunk.block_entities());
    assert_eq!(read, chunk);
}

#[test]
fn a_chunk_that_arrives_with_a_block_entity_of_another_type_than_its_block_has_one_of_the_blocks() {
    let chunk = full();
    let mut wire = chunk_on_wire(&chunk);
    let at = InChunk::new(1, 40, 1);
    let sign = entity_type(blocks::OAK_SIGN).unwrap();
    for (position, entity) in &mut wire.block_entities {
        if *position == at {
            entity.kind = sign;
        }
    }
    let read = chunk_from(&wire).unwrap();
    assert_eq!(
        read.block_entities().get(&at).map(|entity| entity.kind),
        entity_type(blocks::CHEST)
    );
    assert_eq!(read.block_entities().len(), chunk.block_entities().len());
}

#[test]
fn a_chunk_that_arrives_with_the_failure_bit_and_no_version_has_no_failure_bit() {
    let mut wire = chunk_on_wire(&full());
    wire.generated = 0;
    wire.with_failure = true;
    let read = chunk_from(&wire).unwrap();
    assert_eq!(read.generated(), 0);
    assert!(!read.with_failure());
}

/// A chunk, as it travels, with `count` chests, each with `each` bytes of data.
fn with_chests(count: usize, each: usize) -> ChunkOnWire {
    let mut chunk = Chunk::from_sections(0, vec![Section::filled(blocks::AIR, PLAINS); 2]);
    for index in 0..count {
        chunk.set(
            index % 16,
            (index / 256) as i32,
            index / 16 % 16,
            blocks::CHEST,
        );
    }
    let mut wire = chunk_on_wire(&chunk);
    assert_eq!(wire.block_entities.len(), count);
    for (_, entity) in &mut wire.block_entities {
        entity.data = vec![3; each];
    }
    wire
}

#[test]
fn a_chunk_that_arrives_with_more_than_16_kib_of_data_for_one_block_entity_is_an_error() {
    let read = chunk_from(&with_chests(1, 16 * 1024)).unwrap();
    assert_eq!(
        read.block_entities()[&InChunk::new(0, 0, 0)].data.len(),
        16 * 1024
    );
    assert!(chunk_from(&with_chests(1, 16 * 1024 + 1)).is_err());
    assert!(chunk_from(&with_chests(3, 20 * 1024)).is_err());
}

#[test]
fn a_chunk_that_arrives_with_more_than_1_mib_of_block_entity_data_is_an_error() {
    let read = chunk_from(&with_chests(64, 16 * 1024)).unwrap();
    let data: usize = read.block_entities().values().map(|e| e.data.len()).sum();
    assert_eq!(data, 1024 * 1024);

    let mut over = with_chests(65, 16 * 1024);
    over.block_entities[64].1.data = vec![3];
    assert!(chunk_from(&over).is_err());
    over.block_entities[64].1.data = Vec::new();
    assert!(chunk_from(&over).is_ok());
    assert!(chunk_from(&with_chests(100, 16 * 1024)).is_err());
}

#[test]
fn a_chunk_whose_light_has_not_an_entry_below_one_for_each_section_and_one_above_is_an_error() {
    let wire = chunk_on_wire(&full());
    for entries in [0, 4, 5, 7, 26] {
        for sky in [true, false] {
            let mut wire = wire.clone();
            let light = wire.light.as_mut().expect("it has light");
            let list = if sky {
                &mut light.sky
            } else {
                &mut light.block
            };
            list.resize(entries, SectionLight::Dark);
            assert!(chunk_from(&wire).is_err(), "{entries} entries");
        }
    }
    assert!(chunk_from(&wire).is_ok());
}

#[test]
fn light_that_arrives_as_an_array_of_all_15_is_full_and_of_all_0_is_dark() {
    let mut wire = chunk_on_wire(&full());
    let light = wire.light.as_mut().expect("it has light");
    light.sky[3] = SectionLight::Levels(Box::new([0xff; LIGHT_ARRAY_LENGTH]));
    light.block[3] = SectionLight::Levels(Box::new([0; LIGHT_ARRAY_LENGTH]));
    let read = chunk_from(&wire).unwrap();
    let light = read.light().expect("it has light");
    assert_eq!(light.sky[3], SectionLight::Full);
    assert_eq!(light.block[3], SectionLight::Dark);

    let mut same = full();
    let mut expected = some_light(4);
    expected.sky[3] = SectionLight::Full;
    same.set_light(expected);
    assert_eq!(read, same);
}

#[test]
fn a_chunk_with_a_broken_section_or_cut_short_is_an_error_and_not_a_panic() {
    let wire = chunk_on_wire(&full());
    let mut broken = wire.clone();
    let BlocksOnWire::Indexed { words, .. } = &mut broken.sections[1].blocks else {
        panic!("the second section is indexed");
    };
    words.truncate(2000);
    assert!(chunk_from(&broken).is_err());

    let mut broken = wire.clone();
    let BlocksOnWire::Indexed { words, .. } = &mut broken.sections[1].blocks else {
        panic!("the second section is indexed");
    };
    // Forty states at six bits: the highest index is beyond them.
    words[0] = 0xff;
    assert!(chunk_from(&broken).is_err());

    let bytes = bytes_of(&full());
    for length in [0, 1, 4, 100, bytes.len() / 2, bytes.len() - 1] {
        assert!(postcard::from_bytes::<Chunk>(&bytes[..length]).is_err());
    }
}

#[test]
fn a_chunk_with_bytes_changed_at_random_is_an_error_or_a_chunk_that_keeps_its_rules() {
    let bytes = bytes_of(&full());
    let mut numbers = numbers(5);
    for _ in 0..400 {
        let mut bytes = bytes.clone();
        for _ in 0..1 + numbers.next().unwrap() % 3 {
            let at = numbers.next().unwrap() % bytes.len();
            bytes[at] = numbers.next().unwrap() as u8;
        }
        let Ok(chunk) = postcard::from_bytes::<Chunk>(&bytes) else {
            continue;
        };
        // I1's half that can be seen from outside, I2 and the counts of I3, asked
        // in a way that takes a state that is none of the game's.
        assert!(!chunk.with_failure() || chunk.generated() != 0);
        for section in chunk.sections() {
            is_sound(section);
        }
        for (at, entity) in chunk.block_entities() {
            let state = chunk
                .get(usize::from(at.x), i32::from(at.y), usize::from(at.z))
                .expect("a block entity is inside its chunk");
            let kind = state
                .block_id()
                .and_then(|id| BLOCK_INFO[id].block_entity_type);
            assert_eq!(Some(entity.kind), kind);
        }
        if let Some(light) = chunk.light() {
            assert_eq!(light.sky.len(), chunk.sections().len() + 2);
            assert_eq!(light.block.len(), chunk.sections().len() + 2);
        }
    }
}
