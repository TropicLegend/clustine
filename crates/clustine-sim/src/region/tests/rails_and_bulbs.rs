//! Tests of copper bulbs and of rails under power (step R3b of
//! `docs/adr/0026-redstone.md`), from the game's numbers: a bulb toggled by each rising
//! edge of its power, a line of powered rails lit through eight rails from the one
//! with power beside it and not the ninth, and a plain rail at a junction of three
//! turned to the other curve by power beside it.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, with the chunks around chunk (0, 0)
//! loaded so that it ticks.

use clustine_data::{Face, sounds as s};

use super::redstone::{block_at, click, holding, lever, sounds, state, ticking_with};
use super::*;
use crate::region::updates::relative;

const LEVER: BlockPos = BlockPos::new(0, -60, 2);

fn bulb(lit: bool, powered: bool) -> BlockState {
    state(&format!(
        "minecraft:waxed_copper_bulb[lit={lit},powered={powered}]"
    ))
}

/// A copper bulb beside a lever turns on as the lever is pulled, stays on as it is
/// pulled back, and turns off as it is pulled again: each rising edge flips it, with
/// its sound heard by everyone.
#[test]
fn a_copper_bulb_is_toggled_by_each_rising_edge_of_its_power() {
    let at = relative(LEVER, Face::East);
    let mut region = ticking_with(&[(LEVER, lever(false)), (at, bulb(false, false))]);
    let on = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, at), Some(bulb(true, true)));
    assert!(sounds(&on).contains(&(s::BLOCK_COPPER_BULB_TURN_ON, 1.0, 1.0, None)));
    let falling = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, at), Some(bulb(true, false)));
    assert!(
        !sounds(&falling)
            .iter()
            .any(|heard| heard.0 == s::BLOCK_COPPER_BULB_TURN_OFF)
    );
    let off = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, at), Some(bulb(false, true)));
    assert!(sounds(&off).contains(&(s::BLOCK_COPPER_BULB_TURN_OFF, 1.0, 1.0, None)));
    // Lit, its output for comparators is full; dark, none.
    assert_eq!(
        clustine_blocks::signal::analog_output(bulb(true, false)),
        Some(15)
    );
    assert_eq!(
        clustine_blocks::signal::analog_output(bulb(false, true)),
        Some(0)
    );
}

/// A copper bulb placed beside power lights as it is placed (`CopperBulbBlock.onPlace`).
#[test]
fn a_copper_bulb_placed_beside_power_lights() {
    let target = BlockPos::new(2, -60, 2);
    let power = relative(target, Face::East);
    let mut region = ticking_with(&[(power, blocks::REDSTONE_BLOCK)]);
    region.tick(&holding(items::WAXED_COPPER_BULB));
    region.tick(&click(relative(target, Face::Down)));
    assert_eq!(block_at(&region, target), Some(bulb(true, true)));
}

fn powered_rail(powered: bool) -> BlockState {
    state(&format!(
        "minecraft:powered_rail[shape=east_west,powered={powered},waterlogged=false]"
    ))
}

/// A lever beside the first of ten powered rails in a line powers it, and the power
/// runs along the line through eight more; the tenth stays unpowered. Pulled back, the
/// whole line goes.
#[test]
fn a_powered_rail_is_lit_through_eight_rails_and_not_the_ninth() {
    let mut blocks = vec![(LEVER, lever(false))];
    for x in 1..=10 {
        blocks.push((BlockPos::new(x, -60, 2), powered_rail(false)));
    }
    let mut region = ticking_with(&blocks);
    region.tick(&click(LEVER));
    let powered: Vec<bool> = (1..=10)
        .map(|x| {
            block_at(&region, BlockPos::new(x, -60, 2)).and_then(|rail| rail.property("powered"))
                == Some("true")
        })
        .collect();
    let mut expected = vec![true; 9];
    expected.push(false);
    assert_eq!(powered, expected);

    region.tick(&click(LEVER));
    for x in 1..=10 {
        assert_eq!(
            block_at(&region, BlockPos::new(x, -60, 2)),
            Some(powered_rail(false)),
            "at {x}"
        );
    }
}

/// An activator rail is not powered through a powered rail: power runs only along
/// rails of one kind.
#[test]
fn power_runs_along_rails_of_one_kind_only() {
    let activator =
        state("minecraft:activator_rail[shape=east_west,powered=false,waterlogged=false]");
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (BlockPos::new(1, -60, 2), powered_rail(false)),
        (BlockPos::new(2, -60, 2), activator),
    ]);
    region.tick(&click(LEVER));
    assert_eq!(
        block_at(&region, BlockPos::new(1, -60, 2)),
        Some(powered_rail(true))
    );
    assert_eq!(block_at(&region, BlockPos::new(2, -60, 2)), Some(activator));
}

fn rail(shape: &str) -> BlockState {
    state(&format!("minecraft:rail[shape={shape},waterlogged=false]"))
}

/// A plain rail at a junction of three, rails to its north, south and east, curves to
/// the south and east unpowered; a lever pulled beside it turns it to the north and
/// east, and pulled back, back (`RailBlock.updateState`, `RailState.place`). The
/// lever tells it of a block that gives power; a redstone block placed beside it would
/// not turn it, as the game tells the neighbours of a block placed of the block that
/// was there, air.
#[test]
fn power_turns_the_corner_of_a_plain_rail_at_a_junction_of_three() {
    let centre = BlockPos::new(3, -60, 3);
    let lever_at = relative(centre, Face::West);
    let mut region = ticking_with(&[
        (centre, rail("south_east")),
        (relative(centre, Face::North), rail("north_south")),
        (relative(centre, Face::South), rail("north_south")),
        (relative(centre, Face::East), rail("east_west")),
        (lever_at, lever(false)),
    ]);
    region.tick(&click(lever_at));
    assert_eq!(block_at(&region, lever_at), Some(lever(true)));
    assert_eq!(block_at(&region, centre), Some(rail("north_east")));
    region.tick(&click(lever_at));
    assert_eq!(block_at(&region, centre), Some(rail("south_east")));
}

/// A redstone block placed beside the junction leaves the rail as it is, as the game
/// does (see above); broken, it tells the rail of itself, and the rail, unpowered,
/// keeps its curve.
#[test]
fn a_redstone_block_placed_beside_a_junction_leaves_its_rail_as_it_is() {
    let centre = BlockPos::new(3, -60, 3);
    let mut region = ticking_with(&[
        (centre, rail("south_east")),
        (relative(centre, Face::North), rail("north_south")),
        (relative(centre, Face::South), rail("north_south")),
        (relative(centre, Face::East), rail("east_west")),
    ]);
    let power = relative(centre, Face::West);
    region.tick(&holding(items::REDSTONE_BLOCK));
    region.tick(&click(relative(power, Face::Down)));
    assert_eq!(block_at(&region, power), Some(blocks::REDSTONE_BLOCK));
    assert_eq!(block_at(&region, centre), Some(rail("south_east")));
    region.tick(&moves(vec![dig(1, power.x, power.y, power.z, 2)]));
    assert_eq!(block_at(&region, centre), Some(rail("south_east")));
}
