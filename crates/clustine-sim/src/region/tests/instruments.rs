//! Tests of note blocks and bells (step R3b of `docs/adr/0026-redstone.md`), from the
//! game's numbers: a note block's note cycled by clicks, its instrument by the block
//! below and a head above, its play by power that rises and its block event and sound;
//! a bell rung by a click and by power, its block event, and its block entity shaking
//! for fifty ticks in the phase of block entities.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, with the chunks around chunk (0, 0)
//! loaded so that it ticks.

use clustine_blocks::data::tick_id;
use clustine_blocks::note::{self, Instrument};
use clustine_data::Face;

use super::redstone::{api_face, block_at, click, holding, lever, sounds, state, ticking_with};
use super::*;
use crate::region::block_entities::BELL_SHAKING_TICKS;
use crate::region::updates::relative;

const LEVER: BlockPos = BlockPos::new(0, -60, 2);
const NOTE: BlockPos = BlockPos::new(1, -60, 2);

fn note_block(note: u8, powered: bool) -> BlockState {
    state(&format!(
        "minecraft:note_block[instrument=harp,note={note},powered={powered}]"
    ))
}

/// The block events a tick sent to clients.
fn block_events(output: &TickOutput) -> Vec<(BlockPos, u16, i32, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEvent {
                position,
                block,
                kind,
                data,
            } => Some((*position, *block, *kind, *data)),
            _ => None,
        })
        .collect()
}

/// Player 1 clicks the block at `at` on its side `face`, at `height` up the side.
fn click_side(at: BlockPos, face: Face, height: f32) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::UseItemOn {
            position: at,
            face: api_face(face),
            cursor: [0.5, height, 0.5],
            sneaking: false,
            sequence: 1,
        },
    )])
}

/// Each click tunes a note block a semitone up and plays it, in the same tick, as a
/// block event of kind 0 and the harp's sound at the note's pitch, heard by everyone
/// at a volume of 3 from the records; after the highest note, 24, comes the lowest.
#[test]
fn each_click_tunes_a_note_block_one_note_up_and_plays_it() {
    let mut region = ticking_with(&[(NOTE, note_block(0, false))]);
    let output = region.tick(&click(NOTE));
    assert_eq!(block_at(&region, NOTE), Some(note_block(1, false)));
    let block = tick_id(blocks::NOTE_BLOCK);
    assert_eq!(block_events(&output), [(NOTE, block, 0, 0)]);
    let harp = clustine_data::sounds::BLOCK_NOTE_BLOCK_HARP;
    assert_eq!(sounds(&output), [(harp, 3.0, note::pitch(1), None)]);
    let source = output.events.iter().find_map(|event| match event {
        RegionEvent::Sound { source, .. } => Some(*source),
        _ => None,
    });
    assert_eq!(source, Some(crate::api::SOUND_SOURCE_RECORDS));
    for note in 2..=24 {
        region.tick(&click(NOTE));
        assert_eq!(block_at(&region, NOTE), Some(note_block(note, false)));
    }
    let output = region.tick(&click(NOTE));
    assert_eq!(block_at(&region, NOTE), Some(note_block(0, false)));
    assert_eq!(sounds(&output), [(harp, 3.0, 0.5, None)]);
}

/// A note block plays only where the block above it is air: under stone it is tuned
/// and sends no event.
#[test]
fn a_note_block_under_a_block_is_tuned_but_plays_nothing() {
    let above = relative(NOTE, Face::Up);
    let mut region = ticking_with(&[(NOTE, note_block(0, false)), (above, blocks::STONE)]);
    let output = region.tick(&click(NOTE));
    assert_eq!(block_at(&region, NOTE), Some(note_block(1, false)));
    assert_eq!(block_events(&output), []);
    assert_eq!(sounds(&output), []);
}

/// A note block placed takes the instrument of the block below it: the bass drum on
/// stone, the bass on planks; a head clicked onto its top is placed there, and the
/// note block takes the head's sound, which plays at the pitch of 1 whatever the note.
#[test]
fn a_note_block_takes_its_instrument_from_the_block_below_and_a_head_above() {
    let planks = BlockPos::new(3, -60, 2);
    let mut region = ticking_with(&[(planks, state("minecraft:oak_planks"))]);
    region.tick(&holding(items::NOTE_BLOCK));
    let on_stone = BlockPos::new(1, -60, 2);
    region.tick(&click(relative(on_stone, Face::Down)));
    let on_planks = relative(planks, Face::Up);
    region.tick(&click(planks));
    let instrument =
        |region: &Region, pos| Instrument::of_note_block(block_at(region, pos).expect("loaded"));
    assert_eq!(instrument(&region, on_stone), Instrument::Basedrum);
    assert_eq!(instrument(&region, on_planks), Instrument::Bass);

    region.tick(&holding(items::ZOMBIE_HEAD));
    region.tick(&click(on_stone));
    let head = block_at(&region, relative(on_stone, Face::Up)).expect("loaded");
    assert!(
        clustine_blocks::data::same_block(head, blocks::ZOMBIE_HEAD),
        "{head:?}"
    );
    assert_eq!(instrument(&region, on_stone), Instrument::Zombie);
    region.tick(&holding(items::STICK));
    let output = region.tick(&click_side(on_stone, Face::North, 0.5));
    let zombie = clustine_data::sounds::BLOCK_NOTE_BLOCK_IMITATE_ZOMBIE;
    assert_eq!(sounds(&output), [(zombie, 3.0, 1.0, None)]);
}

/// Power that rises plays a note block once, in the tick the lever beside it is
/// pulled, and the block keeps that it is powered; more power while it is powered,
/// and power that goes, play nothing; power that rises again plays it again.
#[test]
fn power_rising_plays_a_note_block_once_and_not_again_while_it_is_powered() {
    let mut region = ticking_with(&[(LEVER, lever(false)), (NOTE, note_block(5, false))]);
    let output = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, NOTE), Some(note_block(5, true)));
    let block = tick_id(blocks::NOTE_BLOCK);
    assert_eq!(block_events(&output), [(NOTE, block, 0, 0)]);
    let harp = clustine_data::sounds::BLOCK_NOTE_BLOCK_HARP;
    assert!(sounds(&output).contains(&(harp, 3.0, note::pitch(5), None)));

    // A redstone block placed beside it while the lever holds it powered.
    region.tick(&holding(items::REDSTONE_BLOCK));
    let beside = relative(NOTE, Face::East);
    let output = region.tick(&click(relative(beside, Face::Down)));
    assert_eq!(block_at(&region, beside), Some(blocks::REDSTONE_BLOCK));
    assert_eq!(block_events(&output), []);
    for _ in 0..3 {
        assert_eq!(block_events(&idle(&mut region)), []);
    }

    // The redstone block broken and the lever pulled back: unpowered, silent.
    region.tick(&moves(vec![dig(1, beside.x, beside.y, beside.z, 2)]));
    let output = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, NOTE), Some(note_block(5, false)));
    assert_eq!(block_events(&output), []);
    let output = region.tick(&click(LEVER));
    assert_eq!(block_events(&output), [(NOTE, block, 0, 0)]);
}

fn floor_bell(powered: bool) -> BlockState {
    state(&format!(
        "minecraft:bell[attachment=floor,facing=north,powered={powered}]"
    ))
}

const BELL: BlockPos = BlockPos::new(1, -60, 2);

/// A bell on the floor facing north rings when it is hit on its north or south side,
/// below its top: it is heard by everyone at a volume of 2, sends a block event of
/// kind 1 that names the side it was hit on, and shakes. Hit on its east side, or on
/// its top, it does not ring.
#[test]
fn a_bell_rings_when_it_is_hit_on_a_side_it_swings_from() {
    let mut region = ticking_with(&[(BELL, floor_bell(false))]);
    assert_eq!(region.bell(BELL).map(|bell| bell.shaking), Some(false));

    let output = region.tick(&click_side(BELL, Face::East, 0.5));
    assert_eq!(block_events(&output), []);
    let output = region.tick(&click_side(BELL, Face::North, 0.9));
    assert_eq!(block_events(&output), []);
    let output = region.tick(&click(BELL));
    assert_eq!(block_events(&output), []);

    let output = region.tick(&click_side(BELL, Face::South, 0.5));
    let block = tick_id(blocks::BELL);
    assert_eq!(
        block_events(&output),
        [(BELL, block, 1, Face::South as i32)]
    );
    let bell_use = clustine_data::sounds::BLOCK_BELL_USE;
    assert_eq!(sounds(&output), [(bell_use, 2.0, 1.0, None)]);
    let bell = region.bell(BELL).expect("a bell");
    assert!(bell.shaking);
    assert_eq!(bell.click_direction, Some(Face::South));
}

/// A rung bell shakes for fifty ticks of its ticker, the tick it was rung in the first,
/// and then is still.
#[test]
fn a_rung_bell_shakes_for_fifty_ticks_and_is_still_again() {
    let mut region = ticking_with(&[(BELL, floor_bell(false))]);
    region.tick(&click_side(BELL, Face::North, 0.5));
    assert_eq!(region.bell(BELL).map(|bell| bell.ticks), Some(1));
    for tick in 2..BELL_SHAKING_TICKS {
        idle(&mut region);
        let bell = region.bell(BELL).expect("a bell");
        assert!(bell.shaking, "tick {tick}");
        assert_eq!(bell.ticks, tick);
    }
    idle(&mut region);
    let bell = region.bell(BELL).expect("a bell");
    assert!(!bell.shaking);
    assert_eq!(bell.ticks, 0);
}

/// Power that rises rings a bell the way it faces, once, and the bell keeps that it is
/// powered.
#[test]
fn power_rising_rings_a_bell_the_way_it_faces() {
    let mut region = ticking_with(&[(LEVER, lever(false)), (BELL, floor_bell(false))]);
    let output = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, BELL), Some(floor_bell(true)));
    let block = tick_id(blocks::BELL);
    assert_eq!(
        block_events(&output),
        [(BELL, block, 1, Face::North as i32)]
    );
    assert!(region.bell(BELL).expect("a bell").shaking);
    let output = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, BELL), Some(floor_bell(false)));
    assert_eq!(block_events(&output), []);
}

/// A bell's ticker is in the list from its chunk's load, in the order of the
/// positions; a bell placed joins the list at its end and one broken leaves it, and
/// its ring goes with it.
#[test]
fn a_bells_ticker_joins_as_the_bell_is_placed_and_leaves_as_it_is_broken() {
    let first = BlockPos::new(3, -60, 1);
    let second = BlockPos::new(1, -60, 3);
    let mut region = ticking_with(&[(second, floor_bell(false)), (first, floor_bell(false))]);
    // In the chunk's order: height, then z, then x.
    assert_eq!(region.tickers(), [first, second]);

    let placed = BlockPos::new(2, -60, 2);
    region.tick(&holding(items::BELL));
    region.tick(&click(relative(placed, Face::Down)));
    let bell = block_at(&region, placed).expect("loaded");
    assert_eq!(bell.property("attachment"), Some("floor"), "{bell:?}");
    assert_eq!(region.tickers(), [first, second, placed]);

    region.tick(&moves(vec![dig(1, first.x, first.y, first.z, 2)]));
    assert_eq!(region.tickers(), [second, placed]);
    assert_eq!(region.bell(first), None);
}
