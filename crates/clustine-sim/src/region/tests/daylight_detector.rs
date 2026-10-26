//! Tests of the daylight detector (`docs/groundwork/n8-plan.md`, section 6.4) from the
//! game's arithmetic: its power through a day under the open sky and under a roof,
//! plain and inverted, read only every 20 ticks of game time; a lamp beside it; and a
//! click turning it. The expected powers were worked out apart from this code, in
//! single precision, from `DaylightDetectorBlock.updateSignalStrength`, the cubic
//! Bézier of `EasingType`, the `day` timeline's keyframes and `Mth.cos`'s table.
//!
//! The regions are those of the redstone tests: chunks within two of (0, 0) on a stone
//! floor at y = -61, player 1 at the spawn.

use super::redstone::{block_at, click, state, ticking_with};
use super::*;

/// Where the detector stands, on the floor.
const DETECTOR: BlockPos = BlockPos::new(0, -60, 2);

fn detector(inverted: bool) -> BlockState {
    state(&format!(
        "minecraft:daylight_detector[inverted={inverted},power=0]"
    ))
}

fn power(region: &Region, pos: BlockPos) -> u8 {
    let detector = block_at(region, pos).expect("loaded");
    clustine_blocks::data::number(detector, "power").expect("a detector")
}

fn inverted(region: &Region, pos: BlockPos) -> bool {
    let detector = block_at(region, pos).expect("loaded");
    clustine_blocks::data::flag(detector, "inverted")
}

/// A roof of stone one block above the floor, seven by seven about the detector: the
/// sky reaches it only from the open columns four blocks off, at 11.
fn roof() -> Vec<(BlockPos, BlockState)> {
    (-3..=3)
        .flat_map(|dx| (-3..=3).map(move |dz| (dx, dz)))
        .map(|(dx, dz)| {
            (
                BlockPos::new(DETECTOR.x + dx, -59, DETECTOR.z + dz),
                blocks::STONE,
            )
        })
        .collect()
}

/// Sets the clocks so that the next tick is at the day time `day` of the second day and
/// at a game time the detector reads at (a multiple of 20), and runs it.
fn tick_at_gate(region: &mut Region, day: u64) -> TickOutput {
    let day = crate::time::DAY + day;
    region.clock.day = day - 1;
    region.clock.game_time = 20 * (region.clock.game_time / 20 + 1) + 19;
    let output = region.tick(&TickInputs::default());
    assert_eq!(region.clock.game_time % 20, 0);
    assert_eq!(region.clock.day, day);
    output
}

/// The day times of the tables below.
const DAYS: [u64; 25] = [
    0, 1_000, 2_000, 3_000, 4_000, 5_000, 6_000, 7_000, 8_000, 9_000, 10_000, 11_000, 11_500,
    12_000, 12_500, 13_000, 13_500, 14_000, 16_000, 18_000, 20_000, 22_000, 22_500, 23_000, 23_500,
];

/// The power at each of [`DAYS`] in the open (sky light 15) and under [`roof`] (11),
/// plain and inverted.
const OPEN: [u8; 25] = [
    7, 10, 12, 13, 14, 15, 15, 15, 14, 13, 12, 10, 8, 7, 4, 2, 1, 0, 0, 0, 0, 0, 1, 2, 4,
];
const OPEN_INVERTED: [u8; 25] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 6, 9, 11, 11, 11, 11, 11, 9, 6, 3,
];
const ROOFED: [u8; 25] = [
    5, 7, 9, 10, 10, 11, 11, 11, 10, 10, 9, 7, 6, 5, 3, 1, 0, 0, 0, 0, 0, 0, 0, 1, 3,
];
const ROOFED_INVERTED: [u8; 25] = [
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 7, 10, 13, 15, 15, 15, 15, 15, 13, 10, 7,
];

fn through_a_day(roofed: bool, inverted: bool) -> Vec<u8> {
    let mut blocks = vec![(DETECTOR, detector(inverted))];
    if roofed {
        blocks.extend(roof());
    }
    let mut region = ticking_with(&blocks);
    let sky = region.light_at(DETECTOR).expect("lit").sky;
    assert_eq!(sky, if roofed { 11 } else { 15 });
    DAYS.iter()
        .map(|day| {
            tick_at_gate(&mut region, *day);
            power(&region, DETECTOR)
        })
        .collect()
}

#[test]
fn a_detector_in_the_open_gives_the_suns_power_through_a_day() {
    assert_eq!(through_a_day(false, false), OPEN);
}

#[test]
fn an_inverted_detector_in_the_open_gives_fifteen_less_the_sky_through_a_day() {
    assert_eq!(through_a_day(false, true), OPEN_INVERTED);
}

#[test]
fn a_detector_under_a_roof_gives_less_through_a_day() {
    assert_eq!(through_a_day(true, false), ROOFED);
}

#[test]
fn an_inverted_detector_under_a_roof_never_gives_less_than_four() {
    assert_eq!(through_a_day(true, true), ROOFED_INVERTED);
}

/// The detector reads only at a game time that is a multiple of 20, whatever the day
/// time: its power holds between, and a change of the sky between two readings shows
/// at the next. Here the game time runs 7 ahead of the day time, so it reads at the day
/// times 13 past a multiple of 20; the power in the open falls to 6 at 12,031 and to 5
/// at 12,230, and the detector shows it at 12,033 and 12,233.
#[test]
fn a_detector_reads_the_sky_only_every_twentieth_tick_of_game_time() {
    let mut region = ticking_with(&[(DETECTOR, detector(false))]);
    region.clock.day = 12_012;
    region.clock.game_time = 12_019;
    let mut seen = Vec::new();
    for _ in 0..262 {
        let output = region.tick(&TickInputs::default());
        let day = region.clock.day;
        let now = power(&region, DETECTOR);
        let expected = match day {
            ..12_033 => 7,
            12_033..12_233 => 6,
            _ => 5,
        };
        assert_eq!(now, expected, "at day time {day}");
        let changed = output.events.iter().any(|event| {
            matches!(event, RegionEvent::BlockChanged { position, .. } if *position == DETECTOR)
        });
        if changed {
            assert_eq!(region.clock.game_time % 20, 0, "at day time {day}");
            seen.push(day);
        }
    }
    assert_eq!(seen, [12_013, 12_033, 12_233]);
}

/// A lamp beside a detector is lit by day, by the detector's signal, and stays dark at
/// night; beside an inverted one it is dark by day and lit at night.
#[test]
fn a_lamp_beside_a_detector_is_lit_by_day_and_beside_an_inverted_one_at_night() {
    let lamp = BlockPos::new(DETECTOR.x + 1, DETECTOR.y, DETECTOR.z);
    let lit =
        |region: &Region| block_at(region, lamp).and_then(|l| l.property("lit")) == Some("true");
    for inverted in [false, true] {
        let mut region = ticking_with(&[
            (DETECTOR, detector(inverted)),
            (lamp, blocks::REDSTONE_LAMP),
        ]);
        tick_at_gate(&mut region, 6_000);
        assert_eq!(lit(&region), !inverted, "inverted {inverted}, at noon");
        tick_at_gate(&mut region, 18_000);
        // A lamp goes out four ticks after its power goes.
        for _ in 0..4 {
            region.tick(&TickInputs::default());
        }
        assert_eq!(lit(&region), inverted, "inverted {inverted}, at midnight");
        tick_at_gate(&mut region, 6_000);
        for _ in 0..4 {
            region.tick(&TickInputs::default());
        }
        assert_eq!(
            lit(&region),
            !inverted,
            "inverted {inverted}, at noon again"
        );
    }
}

/// A click inverts a detector and works its power out again at once, without waiting
/// for its twentieth tick; a second click turns it back.
#[test]
fn a_click_turns_a_detector_and_its_power_follows_at_once() {
    let lamp = BlockPos::new(DETECTOR.x + 1, DETECTOR.y, DETECTOR.z);
    let mut region = ticking_with(&[(DETECTOR, detector(false)), (lamp, blocks::REDSTONE_LAMP)]);
    tick_at_gate(&mut region, 6_000);
    assert_eq!(power(&region, DETECTOR), 15);
    // Off the gate: only the click changes it.
    region.clock.game_time += 5;
    region.tick(&click(DETECTOR));
    assert_ne!(region.clock.game_time % 20, 0);
    assert!(inverted(&region, DETECTOR));
    assert_eq!(power(&region, DETECTOR), 0);
    region.tick(&click(DETECTOR));
    assert!(!inverted(&region, DETECTOR));
    assert_eq!(power(&region, DETECTOR), 15);
    assert_eq!(
        block_at(&region, lamp).and_then(|l| l.property("lit")),
        Some("true")
    );
}
