//! When two chunks are equal and when they show the same, tested from
//! `docs/adr/0022-what-a-chunk-carries.md` (section 2, "When two chunks are equal, said
//! field by field", and the third of its tests) through the crate's public interface.
//!
//! The edge's resume turns on these comparisons. What the edge does with them (a
//! snapshot that differs only in what is not shown sends nothing again) is the edge's
//! to test.

mod common;

use clustine_data::blocks;
use clustine_world::{
    BIOME_CELLS, Biome, Chunk, InChunk, LIGHT_ARRAY_LENGTH, Light, ScheduledTick, Section,
    SectionLight, Ticked, sky_light,
};
use common::*;

/// A chunk with something of everything a chunk carries.
fn full() -> Chunk {
    let mut chunk = Chunk::empty(overworld(), PLAINS);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set(x, 60, z, blocks::STONE);
        }
    }
    chunk.set(1, 61, 1, blocks::WATER);
    chunk.set(2, 61, 2, blocks::CHEST);
    chunk
        .set_block_entity_data(InChunk::new(2, 61, 2), vec![10, 0, 0])
        .unwrap();
    chunk.set(3, 61, 3, blocks::OAK_SIGN);
    chunk.set_ticks(some_ticks());
    chunk.set_marks(some_marks());
    chunk.mark_generated(4, true);
    chunk.set_light(some_light(&chunk, 1));
    chunk
}

/// Fails unless the two are not equal and a client is shown the same of both.
fn differ_only_in_what_is_not_shown(a: &Chunk, b: &Chunk) {
    assert_ne!(a, b);
    assert_ne!(b, a);
    assert!(a.shows_the_same(b) && b.shows_the_same(a));
}

/// Fails unless a client is shown something else of the two, so that they are not
/// equal either.
fn show_something_else(a: &Chunk, b: &Chunk) {
    assert!(!a.shows_the_same(b) && !b.shows_the_same(a));
    assert_ne!(a, b);
    assert_ne!(b, a);
}

#[test]
fn a_chunk_equals_itself_and_its_copy_and_shows_the_same() {
    let chunk = full();
    assert_eq!(chunk, chunk.clone());
    assert!(chunk.shows_the_same(&chunk.clone()));
    assert_eq!(full(), full());
    let empty = Chunk::empty(overworld(), PLAINS);
    assert_eq!(empty, Chunk::empty(overworld(), PLAINS));
    assert!(empty.shows_the_same(&Chunk::empty(overworld(), PLAINS)));
}

#[test]
fn chunks_that_differ_only_in_marks_are_not_equal_and_show_the_same() {
    let mut other = full();
    other.set_marks(Vec::new());
    differ_only_in_what_is_not_shown(&full(), &other);

    let mut marks = some_marks();
    marks.push(InChunk::new(9, 9, 9));
    other.set_marks(marks);
    differ_only_in_what_is_not_shown(&full(), &other);
}

#[test]
fn chunks_whose_marks_were_given_in_another_order_are_equal() {
    let mut other = full();
    let mut marks = some_marks();
    marks.reverse();
    marks.push(marks[0]);
    other.set_marks(marks);
    assert_eq!(full(), other);
}

#[test]
fn chunks_that_differ_only_in_ticks_are_not_equal_and_show_the_same() {
    let mut other = full();
    other.set_ticks(Vec::new());
    differ_only_in_what_is_not_shown(&full(), &other);

    // Each part of a tick counts, and so does the order generation scheduled them in.
    let first = some_ticks()[0];
    let variants = [
        ScheduledTick {
            at: InChunk::new(4, 40, 5),
            ..first
        },
        ScheduledTick {
            of: Ticked::Fluid(2),
            ..first
        },
        ScheduledTick {
            of: Ticked::Block(1),
            ..first
        },
        ScheduledTick { delay: 6, ..first },
        ScheduledTick {
            priority: 1,
            ..first
        },
    ];
    for variant in variants {
        let mut ticks = some_ticks();
        ticks[0] = variant;
        other.set_ticks(ticks);
        differ_only_in_what_is_not_shown(&full(), &other);
    }
    let mut ticks = some_ticks();
    ticks.reverse();
    other.set_ticks(ticks);
    differ_only_in_what_is_not_shown(&full(), &other);
}

#[test]
fn chunks_that_differ_only_in_light_are_not_equal_and_show_the_same() {
    let chunk = full();

    // One has light and the other none.
    let mut dark = Chunk::clone(&chunk);
    dark.set(0, 200, 0, blocks::TORCH);
    dark.set(0, 200, 0, blocks::AIR);
    assert!(dark.light().is_none());
    // That change ended "as generated" too; a chunk like it but for the light:
    let mut lit = dark.clone();
    lit.set_light(some_light(&lit, 1));
    differ_only_in_what_is_not_shown(&lit, &dark);

    // The version of the rules that made it.
    let mut other = full();
    other.set_light(some_light(&other, 2));
    differ_only_in_what_is_not_shown(&chunk, &other);
    other.set_light(some_light(&other, Light::IMPORTED));
    differ_only_in_what_is_not_shown(&chunk, &other);

    // One level of one block, of the sky's light and of the blocks'.
    for sky in [true, false] {
        let mut light = some_light(&chunk, 1);
        let entry = if sky {
            &mut light.sky[1]
        } else {
            &mut light.block[1]
        };
        let SectionLight::Levels(levels) = entry else {
            panic!("the light of the tests has levels there");
        };
        levels[2000] ^= 0x10;
        let mut other = full();
        other.set_light(light);
        differ_only_in_what_is_not_shown(&chunk, &other);
    }

    // A section that is dark where the other's is not listed, or full.
    for case in [SectionLight::Dark, SectionLight::Full] {
        let mut light = some_light(&chunk, 1);
        assert_eq!(light.sky[5], SectionLight::Absent);
        light.sky[5] = case;
        let mut other = full();
        other.set_light(light);
        differ_only_in_what_is_not_shown(&chunk, &other);
    }
}

#[test]
fn chunks_that_differ_only_in_generated_are_not_equal_and_show_the_same() {
    let mut other = full();
    other.mark_generated(5, true);
    differ_only_in_what_is_not_shown(&full(), &other);
    other.mark_generated(0, false);
    differ_only_in_what_is_not_shown(&full(), &other);
}

#[test]
fn chunks_that_differ_only_in_the_failure_bit_are_not_equal_and_show_the_same() {
    let mut other = full();
    other.mark_generated(4, false);
    differ_only_in_what_is_not_shown(&full(), &other);
}

#[test]
fn a_chunk_changed_and_changed_back_shows_the_same_and_is_no_longer_equal() {
    // What a region's snapshot is to an edge's replica once a block was changed and
    // changed back: nothing to send again, and yet another chunk.
    let mut other = full();
    other.set(0, 60, 0, blocks::IRON_ORE);
    other.set(0, 60, 0, blocks::STONE);
    assert_eq!(other.generated(), 0);
    differ_only_in_what_is_not_shown(&full(), &other);
}

#[test]
fn chunks_that_differ_in_a_block_do_not_show_the_same() {
    // A block light cannot tell from the other, one a client cannot (cave air), one
    // with a fluid, and another state of the same block.
    for (x, y, z, new) in [
        (0, 60, 0, blocks::IRON_ORE),
        (0, 200, 0, blocks::CAVE_AIR),
        (0, 200, 0, blocks::STONE),
        (1, 61, 1, flowing_water()),
        (2, 61, 2, chest_facing_east()),
    ] {
        let mut other = full();
        assert!(other.set(x, y, z, new).is_some());
        show_something_else(&full(), &other);
    }
}

#[test]
fn chunks_that_differ_only_in_a_block_and_nothing_else_do_not_show_the_same() {
    // Neither as generated, neither with light: the block is all that differs.
    let mut a = Chunk::empty(overworld(), PLAINS);
    let mut b = Chunk::empty(overworld(), PLAINS);
    a.set(5, 5, 5, blocks::STONE);
    b.set(5, 5, 5, blocks::DIRT);
    show_something_else(&a, &b);
    b.set(5, 5, 5, blocks::STONE);
    assert_eq!(a, b);
    assert!(a.shows_the_same(&b));
}

fn with_biome_in_one_cell(section: usize, cell: usize, biome: Biome) -> Chunk {
    let mut sections = vec![Section::filled(blocks::AIR, PLAINS); 24];
    sections[3] = Section::filled(blocks::STONE, PLAINS);
    let mut cells = Box::new([PLAINS; BIOME_CELLS]);
    cells[cell] = biome;
    sections[section].set_biomes(cells);
    Chunk::from_sections(-64, sections)
}

#[test]
fn chunks_that_differ_in_a_biome_do_not_show_the_same() {
    let plain = with_biome_in_one_cell(0, 0, PLAINS);
    for (section, cell) in [(0, 0), (3, 63), (23, 17)] {
        let other = with_biome_in_one_cell(section, cell, Biome(5));
        show_something_else(&plain, &other);
        assert!(other.shows_the_same(&with_biome_in_one_cell(section, cell, Biome(5))));
    }
    show_something_else(
        &Chunk::empty(overworld(), PLAINS),
        &Chunk::empty(overworld(), Biome(1)),
    );
}

#[test]
fn chunks_that_differ_in_a_block_entity_do_not_show_the_same() {
    // In its data, the blocks being the same.
    let at = InChunk::new(2, 61, 2);
    let mut other = full();
    other.set_block_entity_data(at, vec![10, 0, 1]).unwrap();
    show_something_else(&full(), &other);
    other.set_block_entity_data(at, Vec::new()).unwrap();
    show_something_else(&full(), &other);
    other.set_block_entity_data(at, vec![10, 0, 0]).unwrap();
    assert_eq!(full(), other);

    // In that one has data where the other has none, of another entity.
    let mut other = full();
    other
        .set_block_entity_data(InChunk::new(3, 61, 3), vec![1])
        .unwrap();
    show_something_else(&full(), &other);
}

#[test]
fn chunks_that_differ_in_their_lowest_y_do_not_show_the_same() {
    let sections = vec![Section::filled(blocks::STONE, PLAINS); 4];
    let a = Chunk::from_sections(0, sections.clone());
    let b = Chunk::from_sections(-16, sections);
    show_something_else(&a, &b);
}

#[test]
fn chunks_of_the_same_blocks_are_equal_however_they_came_to_them() {
    // One section went through every form of its blocks and back.
    let mut worn = full();
    let mut id = 100;
    for at in 0..300 {
        let state = clustine_data::BlockState(id);
        worn.set(at % 16, 100 + (at / 256) as i32, at / 16 % 16, state);
        id += 97;
    }
    show_something_else(&full(), &worn);
    for at in 0..300 {
        worn.set(at % 16, 100 + (at / 256) as i32, at / 16 % 16, blocks::AIR);
    }
    assert!(worn.shows_the_same(&full()) && full().shows_the_same(&worn));

    // And it equals a chunk that has the same of everything else, which only one
    // never changed since can be: both are given what the changes took.
    let mut same = full();
    same.set(0, 60, 0, blocks::AIR);
    same.set(0, 60, 0, blocks::STONE);
    for chunk in [&mut worn, &mut same] {
        chunk.mark_generated(4, true);
        chunk.set_light(some_light(chunk, 1));
    }
    assert_eq!(worn, same);
    assert_eq!(worn, full());
}

#[test]
fn a_chunk_for_viewers_has_no_ticks_and_no_marks_and_shows_the_same() {
    let chunk = full();
    let shown = chunk.for_viewers();
    assert!(shown.ticks().is_empty());
    assert!(shown.marks().is_empty());
    assert!(shown.shows_the_same(&chunk) && chunk.shows_the_same(&shown));
    assert_ne!(shown, chunk, "it lacks the ticks and the marks");
    // The chunk itself is as it was.
    assert_eq!(chunk, full());
}

#[test]
fn a_chunk_for_viewers_is_a_copy_but_for_ticks_and_marks() {
    let chunk = full();
    let shown = chunk.for_viewers();
    assert_eq!(shown.generated(), 4);
    assert!(shown.with_failure());
    assert_eq!(shown.light(), chunk.light());
    assert_eq!(shown.block_entities(), chunk.block_entities());
    assert_eq!(shown.heights(), chunk.heights());
    assert_eq!(shown.min_y(), chunk.min_y());
    assert_eq!(shown.sections(), chunk.sections());

    let mut bare = full();
    bare.set_ticks(Vec::new());
    bare.set_marks(Vec::new());
    assert_eq!(shown, bare);
    // Of a chunk without ticks and marks it is the chunk.
    assert_eq!(bare.for_viewers(), bare);
    assert_eq!(shown.for_viewers(), shown);
}

#[test]
fn light_of_an_array_that_is_all_0_is_dark_and_of_one_that_is_all_15_is_full() {
    assert_eq!(
        SectionLight::of(Box::new([0; LIGHT_ARRAY_LENGTH])),
        SectionLight::Dark
    );
    assert_eq!(
        SectionLight::of(Box::new([0xff; LIGHT_ARRAY_LENGTH])),
        SectionLight::Full
    );
    let mut levels = Box::new([0xff; LIGHT_ARRAY_LENGTH]);
    levels[LIGHT_ARRAY_LENGTH - 1] = 0xef;
    assert_eq!(
        SectionLight::of(levels.clone()),
        SectionLight::Levels(levels)
    );
    let mut levels = Box::new([0; LIGHT_ARRAY_LENGTH]);
    levels[0] = 1;
    assert_eq!(
        SectionLight::of(levels.clone()),
        SectionLight::Levels(levels)
    );
}

/// "An array that is all 0 is `Dark` and one that is all 15 is `Full`, always, so that
/// equal light is equal": two chunks with the same light are equal whichever way the
/// light was written down by whoever gave it.
#[test]
fn chunks_given_the_same_light_as_an_array_and_as_full_or_dark_are_equal() {
    let mut as_cases = full();
    let mut light = some_light(&as_cases, 1);
    light.sky[3] = SectionLight::Full;
    light.block[3] = SectionLight::Dark;
    as_cases.set_light(light.clone());

    let mut as_arrays = full();
    light.sky[3] = SectionLight::Levels(Box::new([0xff; LIGHT_ARRAY_LENGTH]));
    light.block[3] = SectionLight::Levels(Box::new([0; LIGHT_ARRAY_LENGTH]));
    as_arrays.set_light(light);

    assert_eq!(as_cases, as_arrays);
    let kept = as_arrays.light().expect("it was given light");
    assert_eq!(kept.sky[3], SectionLight::Full);
    assert_eq!(kept.block[3], SectionLight::Dark);
}

/// The same of the sky light that is reckoned from the heights until light is made:
/// above flat ground a whole section is 15 throughout.
#[test]
fn reckoned_sky_light_that_is_15_throughout_a_section_is_full() {
    let mut chunk = of_air(0, 4);
    // Ground that fills the lowest section exactly: the next one is all sky.
    for x in 0..16 {
        for z in 0..16 {
            chunk.set(x, 15, z, blocks::STONE);
        }
    }
    let sky = sky_light(&chunk);
    assert_eq!(sky.len(), 6, "below, four sections, above");
    assert_eq!(sky[2], SectionLight::Full, "the section above the ground");
    assert!(
        sky.iter().all(|section| match section {
            SectionLight::Levels(levels) =>
                levels.iter().any(|byte| *byte != 0) && levels.iter().any(|byte| *byte != 0xff),
            _ => true,
        }),
        "an array that is all 0 or all 15 is not kept as an array"
    );
}
