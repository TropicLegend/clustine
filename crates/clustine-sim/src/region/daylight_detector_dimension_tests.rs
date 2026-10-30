//! The daylight detector in each level (`docs/groundwork/p-plan.md`, section 17.2 D1;
//! n8 `DaylightDetectorBlock`): in the overworld by its day; in the End, a level with a
//! sky and a fixed time, by its sky light alone at any time (no darkening, the sun at
//! noon); in the Nether, without a sky, never by a tick, and by a click from no sky
//! light. The expected powers are worked out from `updateSignalStrength`: with the sun
//! at 0° the angle is drawn to 0 and `cos` is 1, so the power is the effective sky
//! brightness, or 15 less it inverted.

use clustine_blocks::data::{flag, number};
use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, ChunkPos, Dimension, PlayerId};

use super::super::Region;
use super::super::light::dimension_tests::{DIMENSIONS, floor_y, world_in};
use crate::api::{TickInputs, TickOutput};

fn spot(dimension: Dimension) -> BlockPos {
    BlockPos::new(0, floor_y(dimension) + 1, 2)
}

fn detector(inverted: bool, power: u8) -> BlockState {
    BlockState::parse(&format!(
        "minecraft:daylight_detector[inverted={inverted},power={power}]"
    ))
    .expect("a detector's state")
}

/// A roof of stone one block above the detector, seven by seven about it: the sky
/// reaches it only from the open columns four blocks off, at 11.
fn roof(dimension: Dimension) -> Vec<(BlockPos, BlockState)> {
    let at = spot(dimension);
    (-3..=3)
        .flat_map(|dx| (-3..=3).map(move |dz| (dx, dz)))
        .map(|(dx, dz)| (BlockPos::new(at.x + dx, at.y + 1, at.z + dz), blocks::STONE))
        .collect()
}

fn square() -> Vec<ChunkPos> {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A world of `dimension` with the detector `state`, roofed or not.
fn with_detector(dimension: Dimension, state: BlockState, roofed: bool) -> Region {
    let mut placed = vec![(spot(dimension), state)];
    if roofed {
        placed.extend(roof(dimension));
    }
    let region = world_in(dimension, &square(), &placed);
    assert!(region.ticks_chunk(spot(dimension).chunk()));
    region
}

fn read(region: &Region, dimension: Dimension) -> (bool, u8) {
    let at = spot(dimension);
    let (x, z) = at.in_chunk();
    let state = region
        .chunk(at.chunk())
        .expect("loaded")
        .get(x, at.y, z)
        .expect("a block there");
    (
        flag(state, "inverted"),
        number(state, "power").expect("a detector"),
    )
}

/// Sets the clocks so that the next tick is at the day time `day` of the second day and
/// at a game time the detector reads at (a multiple of 20), and runs it.
fn tick_at_gate(region: &mut Region, day: u64) {
    let day = crate::time::DAY + day;
    region.clock.day = day - 1;
    region.clock.game_time = 20 * (region.clock.game_time / 20 + 1) + 19;
    region.tick(&TickInputs::default());
    assert_eq!(region.clock.game_time % 20, 0);
    assert_eq!(region.clock.day, day);
}

/// A click by a player the region does not hold, as the region's click arm calls it.
fn click(region: &mut Region, dimension: Dimension) {
    let at = spot(dimension);
    let (x, z) = at.in_chunk();
    let state = region
        .chunk(at.chunk())
        .expect("loaded")
        .get(x, at.y, z)
        .expect("a block there");
    let mut output = TickOutput::default();
    let nobody = PlayerId(uuid::Uuid::from_u128(77));
    assert!(region.daylight_detector_used(at, state, nobody, &mut output));
}

/// The power at noon and at midnight, plain and inverted, in the open and under the
/// roof, of a detector that starts at power 7 so that one never ticked shows.
fn noon_and_midnight(dimension: Dimension) -> [[u8; 2]; 4] {
    let mut table = [[0; 2]; 4];
    for (row, (inverted, roofed)) in [(false, false), (true, false), (false, true), (true, true)]
        .into_iter()
        .enumerate()
    {
        let mut region = with_detector(dimension, detector(inverted, 7), roofed);
        let sky = region.light_at(spot(dimension)).expect("lit").sky;
        let expected_sky = match (dimension, roofed) {
            (Dimension::Nether, _) => 0,
            (_, false) => 15,
            (_, true) => 11,
        };
        assert_eq!(sky, expected_sky, "{dimension:?}, roofed {roofed}");
        for (column, day) in [6_000, 18_000].into_iter().enumerate() {
            tick_at_gate(&mut region, day);
            let (still_inverted, power) = read(&region, dimension);
            assert_eq!(still_inverted, inverted);
            table[row][column] = power;
        }
    }
    table
}

#[test]
fn a_detector_in_the_overworld_follows_the_day() {
    // Noon, midnight: plain open, inverted open, plain roofed, inverted roofed.
    assert_eq!(
        noon_and_midnight(Dimension::Overworld),
        [[15, 0], [0, 11], [11, 0], [4, 15]]
    );
}

#[test]
fn a_detector_in_the_end_gives_its_sky_light_whole_by_day_and_by_night() {
    assert_eq!(
        noon_and_midnight(Dimension::End),
        [[15, 15], [0, 0], [11, 11], [4, 4]]
    );
}

#[test]
fn a_detector_in_the_nether_never_ticks_and_keeps_its_power() {
    assert_eq!(
        noon_and_midnight(Dimension::Nether),
        [[7, 7], [7, 7], [7, 7], [7, 7]]
    );
}

/// A click works the power out at once in every level, the Nether's from no sky light:
/// 0 plain, 15 inverted.
#[test]
fn a_click_works_a_detectors_power_out_in_every_level() {
    for dimension in DIMENSIONS {
        let mut region = with_detector(dimension, detector(false, 7), false);
        tick_at_gate(&mut region, 6_000);
        let (plain, inverted) = match dimension {
            Dimension::Overworld | Dimension::End => (15, 0),
            Dimension::Nether => (7, 15),
        };
        assert_eq!(read(&region, dimension), (false, plain), "{dimension:?}");
        click(&mut region, dimension);
        assert_eq!(read(&region, dimension), (true, inverted), "{dimension:?}");
        click(&mut region, dimension);
        let back = if dimension == Dimension::Nether {
            0
        } else {
            15
        };
        assert_eq!(read(&region, dimension), (false, back), "{dimension:?}");
    }
}
