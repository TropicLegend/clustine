//! Tests of the way a fluid pushes what is in it (`fluid::flow`, the game's
//! `FlowingFluid.getFlow`), with the numbers worked out from the game's formula: the
//! differences of the neighbours' heights in ninths, as floats, summed as doubles, and
//! the sum made one long.

use clustine_data::{BlockState, blocks};

use super::{World, at, state};
use crate::fluid::{self, FluidState};

fn water(level: u8) -> BlockState {
    blocks::WATER
        .with_property("level", &level.to_string())
        .expect("water has every level")
}

fn lava(level: u8) -> BlockState {
    blocks::LAVA
        .with_property("level", &level.to_string())
        .expect("lava has every level")
}

fn flow_at(world: &mut World, x: i32, y: i32, z: i32) -> [f64; 3] {
    let held = FluidState::of(world.set[&(x, y, z)]);
    fluid::flow(world, held, at(x, y, z))
}

/// `Vec3.normalize` of the game, by hand.
fn unit([x, y, z]: [f64; 3]) -> [f64; 3] {
    let length = (x * x + y * y + z * z).sqrt();
    [x / length, y / length, z / length]
}

#[test]
fn water_flows_away_from_its_source_towards_where_there_is_less() {
    let stone = state("minecraft:stone");
    let mut world = World::placing()
        .floor(63, stone)
        .with(0, 64, 0, water(0))
        .with(1, 64, 0, water(1));
    // West of the water at (1, 64, 0) is the source, eight ninths against its seven:
    // the step west (-1, by which a float is negated exactly) times the difference (7/9 - 8/9) pushes east; the air beside
    // it on stone pushes nothing.
    let difference = 7.0_f32 / 9.0 - 8.0_f32 / 9.0;
    let dx = f64::from(-difference);
    assert!(dx > 0.0);
    assert_eq!(flow_at(&mut world, 1, 64, 0), unit([dx, 0.0, 0.0]));
    assert_eq!(flow_at(&mut world, 1, 64, 0), [1.0, 0.0, 0.0]);
    // The source has the less high water to its east, which pulls it east as well.
    let pulled = f64::from(1.0_f32 * (8.0_f32 / 9.0 - 7.0_f32 / 9.0));
    assert_eq!(flow_at(&mut world, 0, 64, 0), unit([pulled, 0.0, 0.0]));
}

#[test]
fn two_sources_push_water_between_them_along_the_diagonal() {
    let stone = state("minecraft:stone");
    let mut world = World::placing()
        .floor(63, stone)
        .with(0, 64, 0, water(2))
        .with(-1, 64, 0, water(0))
        .with(0, 64, -1, water(0))
        .with(1, 64, 0, stone)
        .with(0, 64, 1, stone);
    // North (z - 1) and west (x - 1) are sources; east and south stone, no fluid.
    let difference = 6.0_f32 / 9.0 - 8.0_f32 / 9.0;
    let d = f64::from(-difference);
    let flow = flow_at(&mut world, 0, 64, 0);
    assert_eq!(flow, unit([d, 0.0, d]));
    assert!((flow[0] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
}

#[test]
fn water_beside_a_drop_flows_towards_it_by_the_water_below() {
    let stone = state("minecraft:stone");
    // East of the water at (0, 64, 0) is air with water of level 1 below it: the
    // difference is its own height less that below less 8/9 (`0.8888889f`).
    let mut world = World::placing()
        .floor(62, stone)
        .with(0, 63, 0, stone)
        .with(0, 64, 0, water(1))
        .with(1, 63, 0, water(1));
    let difference = 7.0_f32 / 9.0 - (7.0_f32 / 9.0 - 0.888_888_9_f32);
    let dx = f64::from(1.0_f32 * difference);
    assert_eq!(flow_at(&mut world, 0, 64, 0), unit([dx, 0.0, 0.0]));
    // Where the block beside blocks the flow (`#blocks_fluid_flow`, a sign say), the
    // water below it does not count.
    let sign = state("minecraft:oak_sign");
    assert!(crate::data::BLOCKS_FLUID_FLOW.contains(sign));
    let mut blocked = World::placing()
        .floor(62, stone)
        .with(0, 63, 0, stone)
        .with(0, 64, 0, water(1))
        .with(1, 64, 0, sign)
        .with(1, 63, 0, water(1));
    assert_eq!(flow_at(&mut blocked, 0, 64, 0), [0.0, 0.0, 0.0]);
}

#[test]
fn falling_water_beside_a_wall_pushes_down_and_ice_is_no_wall() {
    let stone = state("minecraft:stone");
    // Falling water (level 8) with air on stone around it: no difference sideways, so
    // the flow is nothing; beside a sturdy wall it becomes the unit vector of
    // (0, -6, 0), straight down.
    let mut open = World::placing().floor(63, stone).with(0, 64, 0, water(8));
    assert_eq!(flow_at(&mut open, 0, 64, 0), [0.0, 0.0, 0.0]);
    let mut walled = World::placing()
        .floor(63, stone)
        .with(0, 64, 0, water(8))
        .with(0, 64, -1, stone);
    assert_eq!(flow_at(&mut walled, 0, 64, 0), [0.0, -1.0, 0.0]);
    // A wall one higher counts too.
    let mut above = World::placing()
        .floor(63, stone)
        .with(0, 64, 0, water(8))
        .with(1, 65, 0, stone);
    assert_eq!(flow_at(&mut above, 0, 64, 0), [0.0, -1.0, 0.0]);
    // Ice is never a solid face for it.
    let mut iced = World::placing()
        .floor(63, stone)
        .with(0, 64, 0, water(8))
        .with(0, 64, -1, state("minecraft:ice"));
    assert_eq!(flow_at(&mut iced, 0, 64, 0), [0.0, 0.0, 0.0]);
}

#[test]
fn lava_flows_by_the_same_rule_and_water_beside_it_does_not_count() {
    let stone = state("minecraft:stone");
    let mut world = World::placing()
        .floor(63, stone)
        .with(0, 64, 0, lava(2))
        .with(-1, 64, 0, lava(0))
        .with(1, 64, 0, water(0));
    // The lava source to the west pushes east; the water to the east is another fluid
    // and neither pushes nor lets the lava flow its way.
    let dx = f64::from(-(6.0_f32 / 9.0 - 8.0_f32 / 9.0));
    assert_eq!(flow_at(&mut world, 0, 64, 0), unit([dx, 0.0, 0.0]));
    // Air and the empty fluid flow nowhere.
    let mut air = World::placing();
    assert_eq!(
        fluid::flow(&mut air, FluidState::EMPTY, at(0, 64, 0)),
        [0.0, 0.0, 0.0]
    );
}
