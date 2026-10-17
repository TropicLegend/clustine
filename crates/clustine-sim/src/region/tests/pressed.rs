//! Tests of what players press and step on (step R3b of `docs/adr/0026-redstone.md`),
//! from the game's numbers: a stone pressure plate pressed while a player stands on it
//! and let go by its tick twenty ticks after it went down, a weighted plate's power and
//! time, tripwire between two hooks, big dripleaf tilting under a player and held flat
//! by power, and redstone ore lit by a click and by a step.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, with the chunks around chunk (0, 0)
//! loaded so that it ticks.

use clustine_data::{Face, sounds as s};

use super::redstone::{block_at, click, holding, sounds, state, ticking_with};
use super::*;
use crate::region::updates::relative;

/// Player 1 moves to stand at `(x, y, z)`, on the ground.
fn stand(x: f64, y: f64, z: f64) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(x, y, z)),
            rotation: None,
            on_ground: true,
        },
    )])
}

fn lit(region: &Region, pos: BlockPos) -> bool {
    block_at(region, pos).and_then(|lamp| lamp.property("lit")) == Some("true")
}

const PLATE: BlockPos = BlockPos::new(3, -60, 3);

fn stone_plate(powered: bool) -> BlockState {
    state(&format!(
        "minecraft:stone_pressure_plate[powered={powered}]"
    ))
}

/// A stone plate goes down in the tick a player steps onto it, with its click heard
/// by everyone, and powers a lamp beside it; it looks again every twenty ticks while
/// the player stands there, and the look after they step off lets it go, twenty
/// ticks after it went down, with its click off.
#[test]
fn a_stone_plate_is_pressed_while_a_player_stands_on_it_and_let_go_by_its_tick() {
    let lamp = relative(PLATE, Face::East);
    let mut region = ticking_with(&[(PLATE, stone_plate(false)), (lamp, blocks::REDSTONE_LAMP)]);
    let pressed = region.tick(&stand(3.5, -60.0, 3.5));
    assert_eq!(block_at(&region, PLATE), Some(stone_plate(true)));
    assert!(lit(&region, lamp));
    assert_eq!(
        sounds(&pressed),
        [(s::BLOCK_STONE_PRESSURE_PLATE_CLICK_ON, 1.0, 1.0, None)]
    );
    // The player stands on it past its first look, which keeps it down.
    for _ in 1..25 {
        idle(&mut region);
        assert_eq!(block_at(&region, PLATE), Some(stone_plate(true)));
    }
    // Stepped off in the 25th tick after it went down: the next look is at the 40th.
    region.tick(&stand(0.5, -60.0, 0.5));
    for tick in 26..40 {
        idle(&mut region);
        assert_eq!(
            block_at(&region, PLATE),
            Some(stone_plate(true)),
            "tick {tick}"
        );
    }
    let released = idle(&mut region);
    assert_eq!(block_at(&region, PLATE), Some(stone_plate(false)));
    assert_eq!(
        sounds(&released),
        [(s::BLOCK_STONE_PRESSURE_PLATE_CLICK_OFF, 1.0, 1.0, None)]
    );
}

/// A player who stands beside a plate, not over its pressing part, does not press it.
#[test]
fn a_plate_is_not_pressed_by_a_player_beside_it() {
    let mut region = ticking_with(&[(PLATE, stone_plate(false))]);
    region.tick(&stand(4.4, -60.0, 3.5));
    assert_eq!(block_at(&region, PLATE), Some(stone_plate(false)));
}

/// A light weighted plate under one player has the power of one of its fifteen, and
/// looks again after ten ticks; the click is metal.
#[test]
fn a_light_weighted_plate_counts_one_player_and_looks_again_after_ten_ticks() {
    let plate = |power: u8| {
        state(&format!(
            "minecraft:light_weighted_pressure_plate[power={power}]"
        ))
    };
    let mut region = ticking_with(&[(PLATE, plate(0))]);
    let pressed = region.tick(&stand(3.5, -60.0, 3.5));
    assert_eq!(block_at(&region, PLATE), Some(plate(1)));
    assert_eq!(
        sounds(&pressed)[0].0,
        s::BLOCK_METAL_PRESSURE_PLATE_CLICK_ON
    );
    region.tick(&stand(0.5, -60.0, 0.5));
    for _ in 2..10 {
        idle(&mut region);
        assert_eq!(block_at(&region, PLATE), Some(plate(1)));
    }
    idle(&mut region);
    assert_eq!(block_at(&region, PLATE), Some(plate(0)));
}

const WEST_HOOK: BlockPos = BlockPos::new(2, -60, 2);
const EAST_HOOK: BlockPos = BlockPos::new(6, -60, 2);

fn hook(facing: &str, attached: bool, powered: bool) -> BlockState {
    state(&format!(
        "minecraft:tripwire_hook[facing={facing},attached={attached},powered={powered}]"
    ))
}

fn tripwire(powered: bool) -> BlockState {
    state(&format!(
        "minecraft:tripwire[attached=true,east=true,west=true,north=false,south=false,disarmed=false,powered={powered}]"
    ))
}

/// Two hooks facing each other on walls four apart, with tripwire between them.
fn tripwire_line() -> Region {
    let mut blocks = vec![
        (relative(WEST_HOOK, Face::West), blocks::STONE),
        (WEST_HOOK, hook("east", true, false)),
        (EAST_HOOK, hook("west", true, false)),
        (relative(EAST_HOOK, Face::East), blocks::STONE),
        (relative(WEST_HOOK, Face::North), blocks::REDSTONE_LAMP),
    ];
    for x in 3..=5 {
        blocks.push((BlockPos::new(x, -60, 2), tripwire(false)));
    }
    ticking_with(&blocks)
}

/// A player who walks into tripwire between two hooks presses it: the wire and both
/// hooks are powered in that tick, with the hooks' click, and a lamp beside a hook
/// lights. The player steps off, and ten ticks after the wire was pressed it looks
/// again and lets go, and the hooks with it.
#[test]
fn tripwire_between_two_hooks_is_pressed_by_a_player_and_let_go_after_they_leave() {
    let mut region = tripwire_line();
    let wire = BlockPos::new(4, -60, 2);
    let pressed = region.tick(&stand(4.5, -60.0, 2.5));
    assert_eq!(block_at(&region, wire), Some(tripwire(true)));
    assert_eq!(block_at(&region, WEST_HOOK), Some(hook("east", true, true)));
    assert_eq!(block_at(&region, EAST_HOOK), Some(hook("west", true, true)));
    assert!(lit(&region, relative(WEST_HOOK, Face::North)));
    let clicks: Vec<i32> = sounds(&pressed).into_iter().map(|heard| heard.0).collect();
    assert_eq!(
        clicks,
        [s::BLOCK_TRIPWIRE_CLICK_ON, s::BLOCK_TRIPWIRE_CLICK_ON]
    );
    region.tick(&stand(0.5, -60.0, 0.5));
    for tick in 2..10 {
        idle(&mut region);
        assert_eq!(block_at(&region, wire), Some(tripwire(true)), "tick {tick}");
    }
    idle(&mut region);
    assert_eq!(block_at(&region, wire), Some(tripwire(false)));
    assert_eq!(
        block_at(&region, WEST_HOOK),
        Some(hook("east", true, false))
    );
    assert_eq!(
        block_at(&region, EAST_HOOK),
        Some(hook("west", true, false))
    );
}

/// Hooks placed on walls facing each other with string between them attach the line,
/// with the sound of it holding (`TripWireHookBlock.setPlacedBy`, `TripWireBlock`'s
/// `onPlace`).
#[test]
fn hooks_and_string_placed_between_two_walls_attach_the_line() {
    let west_wall = relative(WEST_HOOK, Face::West);
    let east_wall = relative(EAST_HOOK, Face::East);
    let mut region = ticking_with(&[(west_wall, blocks::STONE), (east_wall, blocks::STONE)]);
    // Within reach of both walls, and off the line.
    region.tick(&stand(4.5, -60.0, 0.5));
    region.tick(&holding(items::STRING));
    for x in 3..=5 {
        region.tick(&click(BlockPos::new(x, -61, 2)));
    }
    let wire = block_at(&region, BlockPos::new(4, -60, 2)).expect("loaded");
    assert!(
        clustine_blocks::data::is_class(wire, clustine_data::BlockClass::TripWireBlock),
        "{wire:?}"
    );
    region.tick(&holding(items::TRIPWIRE_HOOK));
    for (wall, face) in [(west_wall, Face::East), (east_wall, Face::West)] {
        region.tick(&moves(vec![numbered(
            1,
            PlayerInput::UseItemOn {
                position: wall,
                face: super::redstone::api_face(face),
                cursor: [0.5; 3],
                sneaking: false,
                sequence: 1,
            },
        )]));
    }
    assert_eq!(
        block_at(&region, WEST_HOOK),
        Some(hook("east", true, false))
    );
    assert_eq!(
        block_at(&region, EAST_HOOK),
        Some(hook("west", true, false))
    );
    for x in 3..=5 {
        let wire = block_at(&region, BlockPos::new(x, -60, 2)).expect("loaded");
        assert_eq!(wire.property("attached"), Some("true"), "at {x}: {wire:?}");
        assert_eq!(wire.property("east"), Some("true"), "at {x}: {wire:?}");
        assert_eq!(wire.property("west"), Some("true"), "at {x}: {wire:?}");
    }
}

const LEAF: BlockPos = BlockPos::new(3, -60, 3);

fn leaf(tilt: &str) -> BlockState {
    state(&format!("minecraft:big_dripleaf[facing=north,tilt={tilt}]"))
}

/// The leaf's top, where a player stands on it.
const ON_LEAF: f64 = -60.0 + 15.0 / 16.0;

/// A player who stands on a big dripleaf tips it: unstable at once, partial ten ticks
/// on and full ten after that, with the sound of tipping down; a hundred ticks later
/// it is flat again.
#[test]
fn big_dripleaf_tips_under_a_player_by_its_own_ticks() {
    let mut region = ticking_with(&[
        (relative(LEAF, Face::Down), blocks::DIRT),
        (LEAF, leaf("none")),
    ]);
    region.tick(&stand(3.5, ON_LEAF, 3.5));
    assert_eq!(block_at(&region, LEAF), Some(leaf("unstable")));
    for _ in 1..10 {
        idle(&mut region);
    }
    let partial = idle(&mut region);
    assert_eq!(block_at(&region, LEAF), Some(leaf("partial")));
    assert_eq!(sounds(&partial)[0].0, s::BLOCK_BIG_DRIPLEAF_TILT_DOWN);
    // The player falls off, as the client would have them.
    region.tick(&stand(0.5, -60.0, 0.5));
    for _ in 2..10 {
        idle(&mut region);
    }
    idle(&mut region);
    assert_eq!(block_at(&region, LEAF), Some(leaf("full")));
    for _ in 1..100 {
        idle(&mut region);
    }
    let flat = idle(&mut region);
    assert_eq!(block_at(&region, LEAF), Some(leaf("none")));
    assert_eq!(sounds(&flat)[0].0, s::BLOCK_BIG_DRIPLEAF_TILT_UP);
}

/// Power beside a tipped dripleaf lays it flat at once, and holds it flat: a player on
/// it no longer tips it while the power lasts.
#[test]
fn power_lays_a_big_dripleaf_flat_and_holds_it_there() {
    let lever_at = relative(LEAF, Face::West);
    let mut region = ticking_with(&[
        (relative(LEAF, Face::Down), blocks::DIRT),
        (LEAF, leaf("none")),
        (lever_at, super::redstone::lever(false)),
    ]);
    region.tick(&stand(3.5, ON_LEAF, 3.5));
    assert_eq!(block_at(&region, LEAF), Some(leaf("unstable")));
    region.tick(&click(lever_at));
    assert_eq!(block_at(&region, LEAF), Some(leaf("none")));
    for _ in 0..30 {
        idle(&mut region);
        assert_eq!(block_at(&region, LEAF), Some(leaf("none")));
    }
}

/// Redstone ore lights when a player clicks it, and when a player stands on it.
#[test]
fn redstone_ore_lights_when_clicked_and_when_stood_on() {
    let ore = BlockPos::new(3, -60, 3);
    let lit_ore = state("minecraft:redstone_ore[lit=true]");
    let mut region = ticking_with(&[(ore, blocks::REDSTONE_ORE)]);
    region.tick(&holding(items::STICK));
    region.tick(&click(ore));
    assert_eq!(block_at(&region, ore), Some(lit_ore));

    let under = BlockPos::new(5, -61, 5);
    let mut region = ticking_with(&[(under, blocks::REDSTONE_ORE)]);
    region.tick(&stand(5.5, -60.0, 5.5));
    assert_eq!(block_at(&region, under), Some(lit_ore));
}
