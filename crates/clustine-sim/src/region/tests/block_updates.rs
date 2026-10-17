//! Tests of how one change of a block makes others in a region (step B3 of ADR-0024,
//! section 1), written from the specification of the step and from the game's
//! `Level.setBlock`, `Level.destroyBlock` and `CollectingNeighborUpdater`, by someone
//! who did not write the code.
//!
//! Player 1 stands at (0.5, -60, 0.5) on the stone floor of chunk (0, 0). A test that
//! breaks a block has the eight chunks around loaded too, so that chunk (0, 0) ticks,
//! where the updates of blocks whose rules read a signal run (ADR-0026, section 1).
//! Every block a test breaks is within their reach.

use super::*;

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// The state the game would print as `text`.
fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

fn set(position: BlockPos, state: BlockState) -> RegionEvent {
    RegionEvent::BlockChanged {
        position,
        state,
        seen: true,
    }
}

/// A block shown breaking: by the player with this number, or by an update if none.
fn destroyed(position: BlockPos, state: BlockState, breaker: Option<u128>) -> RegionEvent {
    RegionEvent::BlockDestroyed {
        position,
        state,
        breaker: breaker.map(player),
    }
}

/// The region with `blocks` set on the floor of chunk (0, 0), which ticks, after player
/// 1 has broken the block at `broken`, and what that tick reported.
fn breaking(blocks: &[(BlockPos, BlockState)], broken: BlockPos) -> (Region, TickOutput) {
    let mut region = on_ticking_floor_with(blocks);
    let output = region.tick(&moves(vec![dig(1, broken.x, broken.y, broken.z, 1)]));
    (region, output)
}

/// The positions of the blocks a tick changed, in order.
fn changed_positions(output: &TickOutput) -> Vec<BlockPos> {
    block_changes(output)
        .into_iter()
        .map(|event| match event {
            RegionEvent::BlockChanged { position, .. } => position,
            other => panic!("not a change of a block: {other:?}"),
        })
        .collect()
}

/// The blocks a tick showed breaking, each with who broke it.
fn destructions(output: &TickOutput) -> Vec<(BlockPos, Option<PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockDestroyed {
                position, breaker, ..
            } => Some((*position, *breaker)),
            _ => None,
        })
        .collect()
}

fn block_at(region: &Region, position: BlockPos) -> Option<BlockState> {
    let (x, z) = position.in_chunk();
    region.chunk(position.chunk())?.get(x, position.y, z)
}

/// Section 1: the stone under a torch goes, and a fence beside it lets go of it. The
/// player's break is shown first, then the block changes; then the neighbours'
/// shapes are asked again west, east, north, south, down, up, so the fence (east)
/// changes before the torch (up), which an update destroys and so shows breaking.
#[test]
fn breaking_a_block_under_a_torch_beside_a_fence_changes_them_in_the_games_order() {
    let stone = at(2, -60, 3);
    let torch = at(2, -59, 3);
    let fence = at(3, -60, 3);
    let joined = state("minecraft:oak_fence[west=true]");
    let (region, output) = breaking(
        &[
            (stone, blocks::STONE),
            (torch, blocks::TORCH),
            (fence, joined),
        ],
        stone,
    );
    assert_eq!(
        output.events,
        [
            destroyed(stone, blocks::STONE, Some(1)),
            set(stone, blocks::AIR),
            set(fence, blocks::OAK_FENCE),
            destroyed(torch, blocks::TORCH, None),
            set(torch, blocks::AIR),
        ]
    );
    assert_eq!(block_at(&region, fence), Some(blocks::OAK_FENCE));
    assert_eq!(block_at(&region, torch), Some(blocks::AIR));
}

/// Section 1: the shapes of the six neighbours are asked in the order west, east,
/// north, south, down, up; with a block resting on each side, they go in that order.
#[test]
fn the_blocks_resting_on_all_six_sides_of_a_broken_block_go_west_east_north_south_down_up() {
    let held = at(2, -59, 3);
    let west = at(1, -59, 3);
    let east = at(3, -59, 3);
    let north = at(2, -59, 2);
    let south = at(2, -59, 4);
    let down = at(2, -60, 3);
    let up = at(2, -58, 3);
    let (_, output) = breaking(
        &[
            (held, blocks::STONE),
            (west, state("minecraft:wall_torch[facing=west]")),
            (east, state("minecraft:wall_torch[facing=east]")),
            (north, state("minecraft:wall_torch[facing=north]")),
            (south, state("minecraft:wall_torch[facing=south]")),
            (down, state("minecraft:lantern[hanging=true]")),
            (up, blocks::TORCH),
        ],
        held,
    );
    assert_eq!(
        changed_positions(&output),
        [held, west, east, north, south, down, up]
    );
    let by_updates: Vec<_> = [west, east, north, south, down, up]
        .into_iter()
        .map(|position| (position, None))
        .collect();
    assert_eq!(destructions(&output)[1..], by_updates);
}

/// Section 1, the updater: what an update makes while it runs goes before the rest
/// of what made it (depth first). The broken block holds a wall torch on its west and
/// one on its east; a carpet lies on the western torch (a carpet stays on anything
/// that is not air). The western torch goes first, and the carpet with it, before the
/// eastern torch: breadth first would take the eastern torch before the carpet.
#[test]
fn updates_run_depth_first_so_what_one_destroys_goes_before_its_siblings() {
    let held = at(2, -60, 3);
    let western = at(1, -60, 3);
    let carpet = at(1, -59, 3);
    let eastern = at(3, -60, 3);
    let west_torch = state("minecraft:wall_torch[facing=west]");
    let east_torch = state("minecraft:wall_torch[facing=east]");
    let white_carpet = state("minecraft:white_carpet");
    let (_, output) = breaking(
        &[
            (held, blocks::STONE),
            (western, west_torch),
            (carpet, white_carpet),
            (eastern, east_torch),
        ],
        held,
    );
    assert_eq!(
        output.events,
        [
            destroyed(held, blocks::STONE, Some(1)),
            set(held, blocks::AIR),
            destroyed(western, west_torch, None),
            set(western, blocks::AIR),
            destroyed(carpet, white_carpet, None),
            set(carpet, blocks::AIR),
            destroyed(eastern, east_torch, None),
            set(eastern, blocks::AIR),
        ]
    );
}

/// Where a block that rests on another is put, relative to a stone at (2, -59, 3)
/// that holds it, and the block.
fn resting() -> Vec<(BlockPos, BlockState)> {
    let on_top = at(2, -58, 3);
    let under = at(2, -60, 3);
    let north = at(2, -59, 2);
    let east = at(3, -59, 3);
    let mut cases: Vec<(BlockPos, BlockState)> = [
        "minecraft:torch",
        "minecraft:soul_torch",
        "minecraft:copper_torch",
        "minecraft:redstone_torch",
        "minecraft:oak_sign",
        "minecraft:white_banner",
        "minecraft:stone_pressure_plate",
        "minecraft:oak_pressure_plate",
        "minecraft:light_weighted_pressure_plate",
        "minecraft:stone_button[face=floor]",
        "minecraft:lever[face=floor]",
        "minecraft:lantern[hanging=false]",
        "minecraft:soul_lantern[hanging=false]",
        "minecraft:white_carpet",
        "minecraft:snow[layers=1]",
    ]
    .into_iter()
    .map(|text| (on_top, state(text)))
    .collect();
    for text in [
        "minecraft:lantern[hanging=true]",
        "minecraft:soul_lantern[hanging=true]",
        "minecraft:oak_hanging_sign",
        "minecraft:lever[face=ceiling]",
        "minecraft:stone_button[face=ceiling]",
    ] {
        cases.push((under, state(text)));
    }
    // A block on a wall rests on the block behind it, the opposite of where it faces.
    for (position, facing) in [(north, "north"), (east, "east")] {
        for block in [
            "minecraft:wall_torch",
            "minecraft:soul_wall_torch",
            "minecraft:copper_wall_torch",
            "minecraft:redstone_wall_torch",
            "minecraft:ladder",
            "minecraft:oak_wall_sign",
            "minecraft:white_wall_banner",
        ] {
            cases.push((position, state(&format!("{block}[facing={facing}]"))));
        }
        for block in ["minecraft:lever", "minecraft:stone_button"] {
            let text = format!("{block}[face=wall,facing={facing}]");
            cases.push((position, state(&text)));
        }
    }
    cases
}

/// Section 4: each block that rests on another goes when that one is broken, and is
/// shown breaking to everyone, as an update destroyed it.
#[test]
fn a_block_goes_when_the_block_it_rests_on_is_broken() {
    let held = at(2, -59, 3);
    for (position, resting_state) in resting() {
        let (region, output) = breaking(&[(held, blocks::STONE), (position, resting_state)], held);
        assert_eq!(
            output.events,
            [
                destroyed(held, blocks::STONE, Some(1)),
                set(held, blocks::AIR),
                destroyed(position, resting_state, None),
                set(position, blocks::AIR),
            ],
            "{resting_state:?} at {position:?}"
        );
        assert_eq!(block_at(&region, position), Some(blocks::AIR));
    }
}

/// Section 4: support counts only from the one direction a block rests on. A block
/// broken on another side leaves it as it is.
#[test]
fn a_block_stays_when_a_block_it_does_not_rest_on_is_broken() {
    let held = at(2, -59, 3);
    for (position, resting_state) in resting() {
        // Beside a block that rests above or below; below one that rests on a wall.
        let other = if position.y == held.y {
            position.offset(0, -1, 0)
        } else {
            position.offset(-1, 0, 0)
        };
        let (region, output) = breaking(
            &[
                (held, blocks::STONE),
                (position, resting_state),
                (other, blocks::STONE),
            ],
            other,
        );
        assert_eq!(
            output.events,
            [
                destroyed(other, blocks::STONE, Some(1)),
                set(other, blocks::AIR),
            ],
            "{resting_state:?} at {position:?}, beside {other:?}"
        );
        assert_eq!(block_at(&region, position), Some(resting_state));
    }
}

/// Section 4: a ladder on a wall goes when the wall is broken from behind it.
#[test]
fn a_ladder_goes_when_its_wall_is_broken() {
    let wall = at(2, -60, 3);
    let ladder = at(2, -60, 2);
    let on_wall = state("minecraft:ladder[facing=north]");
    let (region, output) = breaking(&[(wall, blocks::STONE), (ladder, on_wall)], wall);
    assert_eq!(changed_positions(&output), [wall, ladder]);
    assert!(output.events.contains(&destroyed(ladder, on_wall, None)));
    assert_eq!(block_at(&region, ladder), Some(blocks::AIR));
}

/// Section 4: a lever under a ceiling rests on the block above it.
#[test]
fn a_lever_on_a_ceiling_goes_with_the_ceiling() {
    let ceiling = at(2, -58, 3);
    let lever = at(2, -59, 3);
    let hanging = state("minecraft:lever[face=ceiling,facing=north]");
    let (_, output) = breaking(&[(ceiling, blocks::STONE), (lever, hanging)], ceiling);
    assert_eq!(
        output.events,
        [
            destroyed(ceiling, blocks::STONE, Some(1)),
            set(ceiling, blocks::AIR),
            destroyed(lever, hanging, None),
            set(lever, blocks::AIR),
        ]
    );
}

/// Section 4: a hanging lantern goes with the block above it, and stays when the
/// block below it is broken.
#[test]
fn a_hanging_lantern_goes_with_the_block_above_and_not_with_the_block_below() {
    let ceiling = at(2, -58, 3);
    let lantern = at(2, -59, 3);
    let below = at(2, -60, 3);
    let hanging = state("minecraft:lantern[hanging=true]");
    let blocks_there = [
        (ceiling, blocks::STONE),
        (lantern, hanging),
        (below, blocks::STONE),
    ];
    let (region, output) = breaking(&blocks_there, below);
    assert_eq!(changed_positions(&output), [below]);
    assert_eq!(block_at(&region, lantern), Some(hanging));

    let (region, output) = breaking(&blocks_there, ceiling);
    assert_eq!(changed_positions(&output), [ceiling, lantern]);
    assert!(output.events.contains(&destroyed(lantern, hanging, None)));
    assert_eq!(block_at(&region, lantern), Some(blocks::AIR));
}

/// Section 4: a carpet goes when what is below it can no longer hold it.
#[test]
fn a_carpet_goes_with_the_block_it_lies_on() {
    let block = at(2, -60, 3);
    let carpet = at(2, -59, 3);
    let red = state("minecraft:red_carpet");
    let (_, output) = breaking(&[(block, blocks::STONE), (carpet, red)], block);
    assert_eq!(
        output.events,
        [
            destroyed(block, blocks::STONE, Some(1)),
            set(block, blocks::AIR),
            destroyed(carpet, red, None),
            set(carpet, blocks::AIR),
        ]
    );
}

/// Section 4: a candle is not taken away by its support going.
#[test]
fn a_candle_stays_when_the_block_it_stands_on_is_broken() {
    let block = at(2, -60, 3);
    let candle = at(2, -59, 3);
    for text in [
        "minecraft:candle[candles=1]",
        "minecraft:white_candle[candles=4,lit=true]",
    ] {
        let standing = state(text);
        let (region, output) = breaking(&[(block, blocks::STONE), (candle, standing)], block);
        assert_eq!(
            output.events,
            [
                destroyed(block, blocks::STONE, Some(1)),
                set(block, blocks::AIR),
            ],
            "{text}"
        );
        assert_eq!(block_at(&region, candle), Some(standing), "{text}");
    }
}

/// Section 4: rails, repeaters, comparators and wire go by `neighborChanged`, which
/// removes them without the game's level event: nobody is shown them breaking.
#[test]
fn rails_repeaters_comparators_and_wire_go_with_their_block_without_being_shown_breaking() {
    let block = at(2, -60, 3);
    let above = at(2, -59, 3);
    for text in [
        "minecraft:rail",
        "minecraft:powered_rail[shape=north_south]",
        "minecraft:detector_rail",
        "minecraft:activator_rail",
        "minecraft:repeater",
        "minecraft:comparator",
        "minecraft:redstone_wire",
    ] {
        let on_block = state(text);
        let (region, output) = breaking(&[(block, blocks::STONE), (above, on_block)], block);
        assert_eq!(
            output.events,
            [
                destroyed(block, blocks::STONE, Some(1)),
                set(block, blocks::AIR),
                set(above, blocks::AIR),
            ],
            "{text}"
        );
        assert_eq!(block_at(&region, above), Some(blocks::AIR), "{text}");
    }
}

/// Section 1: the neighbours are told (`neighborChanged`) before their shapes are
/// asked again. A rail on the broken block goes before a wall torch on its west side,
/// though the shapes are asked west first and up last.
#[test]
fn neighbours_are_told_before_their_shapes_are_asked_again() {
    let block = at(2, -60, 3);
    let rail = at(2, -59, 3);
    let torch = at(1, -60, 3);
    let on_wall = state("minecraft:wall_torch[facing=west]");
    let (_, output) = breaking(
        &[
            (block, blocks::STONE),
            (rail, state("minecraft:rail")),
            (torch, on_wall),
        ],
        block,
    );
    assert_eq!(
        output.events,
        [
            destroyed(block, blocks::STONE, Some(1)),
            set(block, blocks::AIR),
            set(rail, blocks::AIR),
            destroyed(torch, on_wall, None),
            set(torch, blocks::AIR),
        ]
    );
}

/// Section 2: a block that held a water source and is destroyed by an update leaves
/// the water.
#[test]
fn a_waterlogged_ladder_whose_wall_is_broken_leaves_water() {
    let wall = at(2, -60, 3);
    let ladder = at(2, -60, 2);
    let wet = state("minecraft:ladder[facing=north,waterlogged=true]");
    let (region, output) = breaking(&[(wall, blocks::STONE), (ladder, wet)], wall);
    assert_eq!(
        output.events,
        [
            destroyed(wall, blocks::STONE, Some(1)),
            set(wall, blocks::AIR),
            destroyed(ladder, wet, None),
            set(ladder, blocks::WATER),
        ]
    );
    assert_eq!(block_at(&region, ladder), Some(blocks::WATER));
}

/// Section 6: a player's break is shown breaking, by the player, and then the block
/// becomes what its fluid leaves.
#[test]
fn a_waterlogged_block_a_player_breaks_leaves_water() {
    let fence = at(2, -60, 3);
    let wet = state("minecraft:oak_fence[waterlogged=true]");
    let (region, output) = breaking(&[(fence, wet)], fence);
    assert_eq!(
        output.events,
        [destroyed(fence, wet, Some(1)), set(fence, blocks::WATER)]
    );
    assert_eq!(block_at(&region, fence), Some(blocks::WATER));
}

fn door(half: &str) -> BlockState {
    state(&format!("minecraft:oak_door[half={half}]"))
}

/// Section 5: a door whose ground is broken loses its lower half, which takes the
/// upper half with it; both are destroyed by updates, so everyone sees them break.
#[test]
fn a_door_whose_ground_is_broken_goes_lower_half_first() {
    let ground = at(2, -61, 3);
    let (lower, upper) = (at(2, -60, 3), at(2, -59, 3));
    let (region, output) = breaking(&[(lower, door("lower")), (upper, door("upper"))], ground);
    assert_eq!(
        output.events,
        [
            destroyed(ground, blocks::STONE, Some(1)),
            set(ground, blocks::AIR),
            destroyed(lower, door("lower"), None),
            set(lower, blocks::AIR),
            destroyed(upper, door("upper"), None),
            set(upper, blocks::AIR),
        ]
    );
    assert_eq!(block_at(&region, upper), Some(blocks::AIR));
}

/// Section 5: a door broken at its lower half takes its upper half with it, which an
/// update destroys.
#[test]
fn a_door_broken_at_its_lower_half_loses_its_upper_half_to_an_update() {
    let (lower, upper) = (at(2, -60, 3), at(2, -59, 3));
    let (_, output) = breaking(&[(lower, door("lower")), (upper, door("upper"))], lower);
    assert_eq!(
        output.events,
        [
            destroyed(lower, door("lower"), Some(1)),
            set(lower, blocks::AIR),
            destroyed(upper, door("upper"), None),
            set(upper, blocks::AIR),
        ]
    );
}

/// Section 5: a door broken at its upper half loses its lower half first, and both
/// are shown breaking to everyone but the player who broke them.
///
/// The upper half is shown breaking a second time, to everyone: the specification
/// said never to the breaker, and the tester read the game otherwise, which the
/// game's code bears out. `DoublePlantBlock.preventDropFromBottomPart` sets the lower
/// half with flags 35, whose shape updates go on with flags 2, so the upper half's
/// `updateShape` turns it to air and `destroyBlock` shows it breaking to everyone,
/// the breaker too, before the player's own event for it.
#[test]
fn a_door_broken_at_its_upper_half_is_shown_breaking_to_others_and_once_to_everyone() {
    let (lower, upper) = (at(2, -60, 3), at(2, -59, 3));
    let (region, output) = breaking(&[(lower, door("lower")), (upper, door("upper"))], upper);
    assert_eq!(changed_positions(&output), [lower, upper]);
    assert_eq!(
        destructions(&output),
        [
            (upper, None),
            (lower, Some(player(1))),
            (upper, Some(player(1)))
        ],
        "{:?}",
        output.events
    );
    assert_eq!(block_at(&region, lower), Some(blocks::AIR));
    assert_eq!(block_at(&region, upper), Some(blocks::AIR));
}

fn rose_bush(half: &str) -> BlockState {
    state(&format!("minecraft:rose_bush[half={half}]"))
}

/// Section 5: a tall flower whose ground goes, goes whole, lower half first.
#[test]
fn a_tall_flower_whose_ground_is_broken_goes_whole() {
    let ground = at(2, -61, 3);
    let (lower, upper) = (at(2, -60, 3), at(2, -59, 3));
    let (_, output) = breaking(
        &[
            (ground, state("minecraft:dirt")),
            (lower, rose_bush("lower")),
            (upper, rose_bush("upper")),
        ],
        ground,
    );
    assert_eq!(
        output.events,
        [
            destroyed(ground, state("minecraft:dirt"), Some(1)),
            set(ground, blocks::AIR),
            destroyed(lower, rose_bush("lower"), None),
            set(lower, blocks::AIR),
            destroyed(upper, rose_bush("upper"), None),
            set(upper, blocks::AIR),
        ]
    );
}

/// Section 5: a tall flower broken at its lower half loses its upper half too.
#[test]
fn a_tall_flower_broken_at_its_lower_half_loses_its_upper_half() {
    let (lower, upper) = (at(2, -60, 3), at(2, -59, 3));
    let (_, output) = breaking(
        &[
            (at(2, -61, 3), state("minecraft:dirt")),
            (lower, rose_bush("lower")),
            (upper, rose_bush("upper")),
        ],
        lower,
    );
    assert_eq!(
        output.events,
        [
            destroyed(lower, rose_bush("lower"), Some(1)),
            set(lower, blocks::AIR),
            destroyed(upper, rose_bush("upper"), None),
            set(upper, blocks::AIR),
        ]
    );
}

/// Section 5: a tall flower broken at its upper half loses its lower half first, and
/// is shown breaking as the door is, for the reason its test gives.
#[test]
fn a_tall_flower_broken_at_its_upper_half_loses_its_lower_half_first() {
    let (lower, upper) = (at(2, -60, 3), at(2, -59, 3));
    let (_, output) = breaking(
        &[
            (at(2, -61, 3), state("minecraft:dirt")),
            (lower, rose_bush("lower")),
            (upper, rose_bush("upper")),
        ],
        upper,
    );
    assert_eq!(changed_positions(&output), [lower, upper]);
    assert_eq!(
        destructions(&output),
        [
            (upper, None),
            (lower, Some(player(1))),
            (upper, Some(player(1)))
        ],
        "{:?}",
        output.events
    );
}

/// Section 5: the lower half of a two-block plant that held water leaves water when
/// the player breaks the upper half.
#[test]
fn a_waterlogged_two_block_plant_broken_at_its_upper_half_leaves_water_lower_half_first() {
    let dripleaf = |half: &str| {
        state(&format!(
            "minecraft:small_dripleaf[half={half},waterlogged=true]"
        ))
    };
    let (lower, upper) = (at(2, -60, 3), at(2, -59, 3));
    let (region, output) = breaking(
        &[
            (at(2, -61, 3), state("minecraft:clay")),
            (lower, dripleaf("lower")),
            (upper, dripleaf("upper")),
        ],
        upper,
    );
    assert_eq!(
        block_changes(&output),
        [set(lower, blocks::WATER), set(upper, blocks::WATER)]
    );
    assert_eq!(block_at(&region, lower), Some(blocks::WATER));
    assert_eq!(block_at(&region, upper), Some(blocks::WATER));
}

/// Section 7: fences, walls, panes and bars let go of a neighbour that is broken.
#[test]
fn fences_walls_panes_and_bars_let_go_of_a_broken_neighbour() {
    let joining = at(2, -60, 3);
    let neighbour = at(3, -60, 3);
    for (joined, alone, beside) in [
        (
            "minecraft:oak_fence[east=true]",
            "minecraft:oak_fence",
            "minecraft:stone",
        ),
        (
            "minecraft:oak_fence[east=true]",
            "minecraft:oak_fence",
            "minecraft:oak_fence[west=true]",
        ),
        (
            "minecraft:cobblestone_wall[east=low,up=true]",
            "minecraft:cobblestone_wall[up=true]",
            "minecraft:stone",
        ),
        (
            "minecraft:glass_pane[east=true]",
            "minecraft:glass_pane",
            "minecraft:glass_pane[west=true]",
        ),
        (
            "minecraft:iron_bars[east=true]",
            "minecraft:iron_bars",
            "minecraft:stone",
        ),
    ] {
        let (region, output) = breaking(
            &[(joining, state(joined)), (neighbour, state(beside))],
            neighbour,
        );
        assert_eq!(
            block_changes(&output),
            [set(neighbour, blocks::AIR), set(joining, state(alone))],
            "{joined} beside {beside}"
        );
        assert_eq!(block_at(&region, joining), Some(state(alone)));
        // Letting go is a change of shape, not a destruction.
        assert_eq!(
            destructions(&output),
            [(neighbour, Some(player(1)))],
            "{joined} beside {beside}"
        );
    }
}

/// Section 7: a fence joins a block placed beside it, as a region changes a block the
/// same way it breaks one.
#[test]
fn a_fence_joins_a_block_placed_beside_it() {
    let fence = at(2, -60, 3);
    let mut region = on_floor_with(&[(fence, blocks::OAK_FENCE)]);
    // Stone on top of the floor east of the fence.
    let output = region.tick(&moves(vec![place(1, 3, -61, 3, Face::Top)]));
    let joined = state("minecraft:oak_fence[east=true]");
    assert_eq!(
        block_changes(&output),
        [set(at(3, -60, 3), blocks::STONE), set(fence, joined)]
    );
    assert_eq!(block_at(&region, fence), Some(joined));
}

/// Section 8: an update is made only where the block and its six neighbours are in
/// chunks the region has loaded. A torch at the western rim of chunk (0, 0), whose
/// western neighbour is in chunk (-1, 0), which is not loaded, is not updated when the
/// floor under it is broken; a torch one block in is.
#[test]
fn a_block_with_a_neighbour_in_a_chunk_that_is_not_loaded_is_not_updated() {
    let rim = at(0, -60, 3);
    let inner = at(1, -60, 3);
    let mut region = on_floor_with(&[(rim, blocks::TORCH), (inner, blocks::TORCH)]);
    assert!(region.chunk(ChunkPos::new(-1, 0)).is_none());

    let under_rim = at(0, -61, 3);
    let output = region.tick(&moves(vec![dig(1, 0, -61, 3, 1)]));
    assert_eq!(
        output.events,
        [
            destroyed(under_rim, blocks::STONE, Some(1)),
            set(under_rim, blocks::AIR),
        ]
    );
    assert_eq!(block_at(&region, rim), Some(blocks::TORCH));

    let under_inner = at(1, -61, 3);
    let output = region.tick(&moves(vec![dig(1, 1, -61, 3, 2)]));
    assert_eq!(
        output.events,
        [
            destroyed(under_inner, blocks::STONE, Some(1)),
            set(under_inner, blocks::AIR),
            destroyed(inner, blocks::TORCH, None),
            set(inner, blocks::AIR),
        ]
    );
    // The torch at the rim stays still: nothing asked it again.
    assert_eq!(block_at(&region, rim), Some(blocks::TORCH));
}
