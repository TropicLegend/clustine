//! Tests of the rules against small worlds.

mod fluid_rules;

use std::collections::BTreeMap;

use clustine_data::{BlockState, Face, blocks};
use clustine_world::BlockPos;

use crate::connect;
use crate::{BlockView, Rules, can_survive, update_shape};

/// A world of air with the blocks set in it, which keeps the ticks it is asked for.
struct World {
    set: BTreeMap<(i32, i32, i32), BlockState>,
    light: Option<u8>,
    rules: Rules,
    block_ticks: Vec<(BlockPos, u16, i32)>,
    fluid_ticks: Vec<(BlockPos, u8, i32)>,
}

impl World {
    fn new(rules: Rules) -> Self {
        Self {
            set: BTreeMap::new(),
            light: None,
            rules,
            block_ticks: Vec::new(),
            fluid_ticks: Vec::new(),
        }
    }

    /// A world for generation, whose sky reads 15 everywhere as it does while features
    /// are placed.
    fn generation() -> Self {
        let mut world = Self::new(Rules::Generation);
        world.light = Some(15);
        world
    }

    fn placing() -> Self {
        Self::new(Rules::Placing)
    }

    fn with(mut self, x: i32, y: i32, z: i32, state: BlockState) -> Self {
        self.set.insert((x, y, z), state);
        self
    }

    fn floor(mut self, y: i32, state: BlockState) -> Self {
        for x in -4..8 {
            for z in -4..8 {
                self.set.insert((x, y, z), state);
            }
        }
        self
    }
}

impl BlockView for World {
    fn block(&mut self, at: BlockPos) -> BlockState {
        *self.set.get(&(at.x, at.y, at.z)).unwrap_or(&blocks::AIR)
    }

    fn raw_brightness(&mut self, _at: BlockPos) -> Option<u8> {
        self.light
    }

    fn schedule_block_tick(&mut self, at: BlockPos, block: u16, delay: i32) {
        self.block_ticks.push((at, block, delay));
    }

    fn schedule_fluid_tick(&mut self, at: BlockPos, fluid: u8, delay: i32) {
        self.fluid_ticks.push((at, fluid, delay));
    }

    fn rules(&self) -> Rules {
        self.rules
    }
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("no state {text}"))
}

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// What the block at `pos` in `world` becomes when told of its neighbour on `side`.
fn shape_from(world: &mut World, pos: BlockPos, side: Face) -> BlockState {
    let own = world.block(pos);
    let neighbour = world.block(crate::data::relative(pos, side));
    update_shape(world, own, pos, side, neighbour)
}

#[test]
fn a_flower_stays_on_grass_and_not_on_stone() {
    for mut world in [World::generation(), World::placing()] {
        world = world.floor(64, blocks::GRASS_BLOCK);
        assert!(can_survive(blocks::POPPY, at(0, 65, 0), &mut world));
        world = world.with(0, 64, 0, blocks::STONE);
        assert!(!can_survive(blocks::POPPY, at(0, 65, 0), &mut world));
    }
}

#[test]
fn a_mushroom_on_stone_needs_dim_light_where_the_caller_has_light() {
    let mut lit = World::generation().floor(64, blocks::STONE);
    assert!(!can_survive(blocks::BROWN_MUSHROOM, at(0, 65, 0), &mut lit));
    lit.light = Some(12);
    assert!(can_survive(blocks::BROWN_MUSHROOM, at(0, 65, 0), &mut lit));
    // Placing has no light, and does not judge it.
    let mut dark = World::placing().floor(64, blocks::STONE);
    assert!(can_survive(blocks::BROWN_MUSHROOM, at(0, 65, 0), &mut dark));
    let mut mycelium = World::generation().floor(64, blocks::MYCELIUM);
    assert!(can_survive(
        blocks::BROWN_MUSHROOM,
        at(0, 65, 0),
        &mut mycelium
    ));
}

#[test]
fn coral_stays_on_a_sturdy_top_for_generation_and_is_not_ported_for_placing() {
    let mut generation = World::generation().floor(40, blocks::SAND);
    assert!(can_survive(
        blocks::TUBE_CORAL,
        at(0, 41, 0),
        &mut generation
    ));
    assert!(!can_survive(
        blocks::TUBE_CORAL,
        at(0, 43, 0),
        &mut generation
    ));
    let mut placing = World::placing().floor(40, blocks::SAND);
    assert!(!can_survive(blocks::TUBE_CORAL, at(0, 41, 0), &mut placing));
    // A coral fan is ported for both.
    assert!(can_survive(
        blocks::TUBE_CORAL_FAN,
        at(0, 41, 0),
        &mut placing
    ));
}

#[test]
fn scaffolding_is_held_within_seven_blocks_of_what_holds_it_up_when_placing() {
    let mut world = World::placing().floor(63, blocks::STONE);
    let distance = |d: &str| state(&format!("minecraft:scaffolding[distance={d}]"));
    world = world.with(0, 64, 0, distance("6"));
    assert_eq!(
        crate::survive::scaffolding_distance(at(1, 64, 0), &mut world),
        0
    );
    assert_eq!(
        crate::survive::scaffolding_distance(at(1, 65, 0), &mut world),
        7
    );
    world = world.with(0, 65, 0, distance("6"));
    assert_eq!(
        crate::survive::scaffolding_distance(at(1, 65, 0), &mut world),
        7
    );
    world = world.with(0, 65, 0, distance("5"));
    assert!(can_survive(blocks::SCAFFOLDING, at(1, 65, 0), &mut world));
    world = world.with(0, 65, 0, distance("6"));
    assert!(!can_survive(blocks::SCAFFOLDING, at(1, 65, 0), &mut world));
}

#[test]
fn a_wall_torch_needs_a_sturdy_side_behind_it() {
    let torch = state("minecraft:wall_torch[facing=east]");
    let mut world = World::placing();
    assert!(!can_survive(torch, at(1, 64, 0), &mut world));
    world = world.with(0, 64, 0, blocks::STONE);
    assert!(can_survive(torch, at(1, 64, 0), &mut world));
    world = world.with(0, 64, 0, blocks::OAK_FENCE);
    assert!(!can_survive(torch, at(1, 64, 0), &mut world));
}

#[test]
fn sugar_cane_needs_water_beside_its_ground() {
    let mut world = World::generation().floor(62, blocks::SAND);
    assert!(!can_survive(blocks::SUGAR_CANE, at(5, 63, 5), &mut world));
    world = world.with(6, 62, 5, blocks::WATER);
    assert!(can_survive(blocks::SUGAR_CANE, at(5, 63, 5), &mut world));
}

#[test]
fn leaves_ask_for_a_tick_only_where_a_neighbour_changes_their_distance() {
    let leaves = |distance: u8| state(&format!("minecraft:oak_leaves[distance={distance}]"));
    let mut world = World::generation()
        .with(0, 64, 0, leaves(1))
        .with(1, 64, 0, blocks::OAK_LOG);
    assert_eq!(shape_from(&mut world, at(0, 64, 0), Face::East), leaves(1));
    assert!(world.block_ticks.is_empty());
    world = world.with(0, 64, 0, leaves(3));
    shape_from(&mut world, at(0, 64, 0), Face::East);
    assert_eq!(world.block_ticks.len(), 1);
    assert_eq!(world.block_ticks[0].2, 1);
}

#[test]
fn a_flower_whose_ground_goes_becomes_air() {
    let mut world =
        World::generation()
            .floor(64, blocks::GRASS_BLOCK)
            .with(0, 65, 0, blocks::POPPY);
    assert_eq!(
        shape_from(&mut world, at(0, 65, 0), Face::Down),
        blocks::POPPY
    );
    world = world.with(0, 64, 0, blocks::AIR);
    assert_eq!(
        shape_from(&mut world, at(0, 65, 0), Face::Down),
        blocks::AIR
    );
}

#[test]
fn a_waterlogged_block_lets_its_water_flow_on() {
    let fence = state("minecraft:oak_fence[waterlogged=true]");
    let mut world = World::generation().with(0, 64, 0, fence);
    shape_from(&mut world, at(0, 64, 0), Face::Up);
    assert_eq!(world.fluid_ticks, vec![(at(0, 64, 0), 2, 5)]);
}

#[test]
fn grass_under_snow_is_snowy_and_sand_beside_a_change_asks_to_fall() {
    let mut world = World::generation()
        .with(0, 64, 0, blocks::GRASS_BLOCK)
        .with(0, 65, 0, blocks::SNOW);
    let snowy = shape_from(&mut world, at(0, 64, 0), Face::Up);
    assert_eq!(snowy.property("snowy"), Some("true"));
    let mut world = World::generation().with(0, 64, 0, blocks::SAND);
    assert_eq!(
        shape_from(&mut world, at(0, 64, 0), Face::North),
        blocks::SAND
    );
    assert_eq!(world.block_ticks.len(), 1);
    assert_eq!(world.block_ticks[0].2, 2);
}

#[test]
fn a_vine_without_anything_to_hold_on_to_goes() {
    let vine = state("minecraft:vine[north=true]");
    let mut world = World::generation()
        .with(0, 64, 0, vine)
        .with(0, 64, -1, blocks::STONE);
    assert_eq!(shape_from(&mut world, at(0, 64, 0), Face::North), vine);
    world = world.with(0, 64, -1, blocks::AIR);
    assert_eq!(
        shape_from(&mut world, at(0, 64, 0), Face::North),
        blocks::AIR
    );
}

#[test]
fn a_fence_joins_a_log_beside_it_and_lets_go_of_air() {
    let mut world =
        World::generation()
            .with(0, 64, 0, blocks::OAK_FENCE)
            .with(1, 64, 0, blocks::OAK_LOG);
    let joined = shape_from(&mut world, at(0, 64, 0), Face::East);
    assert_eq!(joined, state("minecraft:oak_fence[east=true]"));
    world = world.with(0, 64, 0, joined).with(1, 64, 0, blocks::AIR);
    assert_eq!(
        shape_from(&mut world, at(0, 64, 0), Face::East),
        blocks::OAK_FENCE
    );
    // Nothing above or below changes a fence.
    world = world.with(0, 65, 0, blocks::STONE);
    assert_eq!(shape_from(&mut world, at(0, 64, 0), Face::Up), joined);
}

#[test]
fn a_fence_joins_its_own_kind_and_not_leaves_or_another_kind() {
    let fence = blocks::OAK_FENCE;
    let east = |neighbour| connect::fence_joins(fence, Face::East, neighbour);
    assert!(east(blocks::SPRUCE_FENCE));
    assert!(!east(blocks::NETHER_BRICK_FENCE));
    assert!(!east(blocks::OAK_LEAVES));
    assert!(!east(blocks::PUMPKIN));
    assert!(east(blocks::STONE));
    // A gate joins what lies at its ends, across its facing.
    let gate = |facing: &str| state(&format!("minecraft:oak_fence_gate[facing={facing}]"));
    assert!(east(gate("north")));
    assert!(!east(gate("east")));
}

#[test]
fn a_pane_joins_bars_walls_and_sturdy_sides() {
    assert!(connect::pane_joins(Face::South, blocks::IRON_BARS));
    assert!(connect::pane_joins(Face::South, blocks::GLASS_PANE));
    assert!(connect::pane_joins(Face::South, blocks::COBBLESTONE_WALL));
    assert!(connect::pane_joins(Face::South, blocks::STONE));
    assert!(!connect::pane_joins(Face::South, blocks::OAK_FENCE));
    let mut world =
        World::generation()
            .with(0, 64, 0, blocks::GLASS_PANE)
            .with(0, 64, 1, blocks::IRON_BARS);
    let joined = shape_from(&mut world, at(0, 64, 0), Face::South);
    assert_eq!(joined.property("south"), Some("true"));
}

#[test]
fn a_wall_runs_low_to_a_wall_beside_it_and_tall_under_a_block() {
    let mut world = World::generation()
        .with(0, 64, 0, blocks::COBBLESTONE_WALL)
        .with(1, 64, 0, blocks::COBBLESTONE_WALL);
    let joined = shape_from(&mut world, at(0, 64, 0), Face::East);
    assert_eq!(joined.property("east"), Some("low"));
    // Joined on one side only, it keeps its post.
    assert_eq!(joined.property("up"), Some("true"));
    world = world.with(0, 64, 0, joined).with(0, 65, 0, blocks::STONE);
    let tall = shape_from(&mut world, at(0, 64, 0), Face::Up);
    assert_eq!(tall.property("east"), Some("tall"));
    assert_eq!(tall.property("west"), Some("none"));
    // The block below changes nothing.
    world = world.with(0, 64, 0, tall);
    assert_eq!(shape_from(&mut world, at(0, 64, 0), Face::Down), tall);
}

#[test]
fn a_straight_wall_drops_its_post_and_a_corner_keeps_it() {
    let east_west = state("minecraft:cobblestone_wall[east=low,west=low,up=true]");
    let mut world = World::generation()
        .with(0, 64, 0, east_west)
        .with(-1, 64, 0, blocks::COBBLESTONE_WALL)
        .with(1, 64, 0, blocks::COBBLESTONE_WALL);
    let straight = shape_from(&mut world, at(0, 64, 0), Face::East);
    assert_eq!(straight.property("up"), Some("false"));
    world = world
        .with(0, 64, 0, straight)
        .with(0, 64, -1, blocks::COBBLESTONE_WALL);
    let corner = shape_from(&mut world, at(0, 64, 0), Face::North);
    assert_eq!(corner.property("north"), Some("low"));
    assert_eq!(corner.property("up"), Some("true"));
}

#[test]
fn a_fence_gate_between_walls_lies_in_the_wall() {
    let gate = state("minecraft:oak_fence_gate[facing=north]");
    let mut world =
        World::generation()
            .with(0, 64, 0, gate)
            .with(1, 64, 0, blocks::COBBLESTONE_WALL);
    let in_wall = shape_from(&mut world, at(0, 64, 0), Face::East);
    assert_eq!(in_wall.property("in_wall"), Some("true"));
    // A wall in front of it is not at its ends.
    let mut world =
        World::generation()
            .with(0, 64, 0, gate)
            .with(0, 64, -1, blocks::COBBLESTONE_WALL);
    assert_eq!(shape_from(&mut world, at(0, 64, 0), Face::North), gate);
    assert!(connect::gate_in_wall(
        at(0, 64, 0),
        Face::South,
        &mut World::generation().with(-1, 64, 0, blocks::COBBLESTONE_WALL)
    ));
}

#[test]
fn stairs_behind_stairs_turned_aside_make_an_outer_corner() {
    let own = state("minecraft:oak_stairs[facing=north,half=bottom]");
    let mut world = World::generation().with(0, 64, 0, own).with(
        0,
        64,
        -1,
        state("minecraft:oak_stairs[facing=east,half=bottom]"),
    );
    let corner = shape_from(&mut world, at(0, 64, 0), Face::North);
    assert_eq!(corner.property("shape"), Some("outer_right"));
    // Stairs of the other half do not.
    world = world.with(
        0,
        64,
        -1,
        state("minecraft:oak_stairs[facing=east,half=top]"),
    );
    let straight = shape_from(&mut world, at(0, 64, 0), Face::North);
    assert_eq!(straight.property("shape"), Some("straight"));
}

#[test]
fn a_cross_of_wire_runs_straight_once_wire_lies_beside_it_and_goes_without_ground() {
    let mut world = World::generation().floor(63, blocks::STONE);
    let alone = connect::redstone_wire(blocks::REDSTONE_WIRE, at(0, 64, 0), &mut world);
    for side in ["north", "east", "south", "west"] {
        assert_eq!(alone.property(side), Some("side"), "{side}");
    }
    world = world
        .with(0, 64, 0, alone)
        .with(1, 64, 0, blocks::REDSTONE_WIRE);
    let straight = shape_from(&mut world, at(0, 64, 0), Face::East);
    assert_eq!(straight.property("east"), Some("side"));
    assert_eq!(straight.property("west"), Some("side"));
    assert_eq!(straight.property("north"), Some("none"));
    assert_eq!(straight.property("south"), Some("none"));
    assert_eq!(
        straight,
        connect::redstone_wire(blocks::REDSTONE_WIRE, at(0, 64, 0), &mut world)
    );
    world = world.with(0, 63, 0, blocks::AIR);
    assert_eq!(
        shape_from(&mut world, at(0, 64, 0), Face::Down),
        blocks::AIR
    );
}

#[test]
fn shapes_from_each_side_in_turn_are_the_shapes_the_blocks_are_placed_with() {
    // The game builds `getStateForPlacement` and `updateShape` of these blocks from the
    // same tests; a block told of each of its sides in turn ends as it would be placed.
    let pos = at(0, 64, 0);
    let surroundings = [
        (1, 64, 0, blocks::OAK_LOG),
        (-1, 64, 0, blocks::COBBLESTONE_WALL),
        (0, 64, 1, blocks::IRON_BARS),
        (0, 64, -1, blocks::SPRUCE_FENCE),
        (0, 65, 0, blocks::STONE),
    ];
    type Place = fn(BlockState, BlockPos, &mut World) -> BlockState;
    let cases: [(BlockState, Place); 3] = [
        (blocks::OAK_FENCE, connect::fence),
        (blocks::GLASS_PANE, connect::pane),
        (blocks::COBBLESTONE_WALL, connect::wall),
    ];
    for (block, place) in cases {
        let mut world = World::generation();
        for (x, y, z, state) in surroundings {
            world = world.with(x, y, z, state);
        }
        let placed = place(block, pos, &mut world);
        let mut updated = block;
        for side in [
            Face::West,
            Face::East,
            Face::North,
            Face::South,
            Face::Down,
            Face::Up,
        ] {
            let neighbour = world.block(crate::data::relative(pos, side));
            updated = update_shape(&mut world, updated, pos, side, neighbour);
        }
        assert_eq!(updated, placed, "{block:?}");
    }
}
