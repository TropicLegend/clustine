//! Tests of water and lava in a region (step B5 of ADR-0024), written from the
//! specification of the game's fluids by someone who did not write the code.
//!
//! Player 1 stands at (0.5, -60, 0.5) on a stone floor at y = -61. The chunks from -2
//! to 2 each way are loaded, so that the nine from -1 to 1 tick: every block from
//! x, z = -16 up to 31 is where fluids flow. Most tests are around (8, -60, 8), on the
//! floor in the middle of chunk (0, 0). A tick of the simulation is a call, so the tests
//! count ticks where the specification names delays.

use clustine_blocks::data::is_class;
use clustine_blocks::fluid::{FLOWING_WATER, LAVA, WATER};
use clustine_data::BlockClass;
use clustine_world::{ScheduledTick, Ticked};

use super::*;

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// The state the game would print as `text`.
fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

fn in_chunk(position: BlockPos) -> InChunk {
    let (x, z) = position.in_chunk();
    InChunk::new(x, position.y, z)
}

fn block_at(region: &Region, position: BlockPos) -> Option<BlockState> {
    let (x, z) = position.in_chunk();
    region.chunk(position.chunk())?.get(x, position.y, z)
}

/// A tick of the fluid `fluid` at `position`, due at once.
fn fluid_tick(position: BlockPos, fluid: u8) -> ScheduledTick {
    ScheduledTick {
        at: in_chunk(position),
        of: Ticked::Fluid(fluid),
        delay: 0,
        priority: 0,
    }
}

/// A tick of the block `block` at `position`, due at once.
fn block_tick(position: BlockPos, block: BlockState) -> ScheduledTick {
    let id = block.block_id().expect("a block state");
    ScheduledTick {
        at: in_chunk(position),
        of: Ticked::Block(u16::try_from(id).expect("a block id fits in 16 bits")),
        delay: 0,
        priority: 0,
    }
}

/// The chunks from -2 to 2 each way, with the floor everywhere and `blocks` set (by
/// their position in the world) and `ticks` kept, loaded for player 1 in one tick.
/// Returns the region and what that tick, in which the ticks due at once run,
/// reported.
fn world_with(
    blocks: &[(BlockPos, BlockState)],
    ticks: &[(BlockPos, ScheduledTick)],
) -> (Region, TickOutput) {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let positions: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let mut chunks: BTreeMap<ChunkPos, Chunk> = positions
        .iter()
        .map(|position| (*position, floor()))
        .collect();
    for (position, block) in blocks {
        let chunk = chunks
            .get_mut(&position.chunk())
            .unwrap_or_else(|| panic!("{position:?} is in a loaded chunk"));
        let (x, z) = position.in_chunk();
        chunk.set(x, position.y, z, *block);
    }
    let mut kept: BTreeMap<ChunkPos, Vec<ScheduledTick>> = BTreeMap::new();
    for (position, tick) in ticks {
        kept.entry(position.chunk()).or_default().push(*tick);
    }
    for (position, ticks) in kept {
        chunks
            .get_mut(&position)
            .expect("a tick in a loaded chunk")
            .set_ticks(ticks);
    }
    checked(&mut region, &tickets(positions, vec![]));
    let output = checked(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks.into_iter().collect(),
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, [], "asked for again");
    (region, output)
}

/// A world with `sources` of the fluid `fluid`, each ticking at once.
fn sources_of(
    fluid: u8,
    sources: &[BlockPos],
    blocks: &[(BlockPos, BlockState)],
) -> (Region, TickOutput) {
    let block = if fluid == LAVA {
        blocks::LAVA
    } else {
        blocks::WATER
    };
    let mut all: Vec<(BlockPos, BlockState)> = blocks.to_vec();
    all.extend(sources.iter().map(|source| (*source, block)));
    let ticks: Vec<(BlockPos, ScheduledTick)> = sources
        .iter()
        .map(|source| (*source, fluid_tick(*source, fluid)))
        .collect();
    world_with(&all, &ticks)
}

fn idle_tick(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

/// Runs `count` ticks with nothing in them, and returns everything they reported.
fn run(region: &mut Region, count: usize) -> Vec<RegionEvent> {
    (0..count).flat_map(|_| idle_tick(region).events).collect()
}

/// The places of the hisses among `events`.
fn fizzes(events: &[RegionEvent]) -> Vec<BlockPos> {
    events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Fizzed { position } => Some(*position),
            _ => None,
        })
        .collect()
}

/// The level of the liquid block at `position`, or `None` where there is none.
fn level_at(region: &Region, position: BlockPos) -> Option<u8> {
    let state = block_at(region, position)?;
    if !is_class(state, BlockClass::LiquidBlock) {
        return None;
    }
    state.property("level").and_then(|level| level.parse().ok())
}

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

/// Sets the block at `position` as a player's action or a bucket would, between two
/// ticks, and returns what that reported.
fn set(region: &mut Region, position: BlockPos, block: BlockState) -> TickOutput {
    let mut output = TickOutput::default();
    region.set_block(position, block, &mut output);
    output
}

const CENTRE: (i32, i32) = (8, 8);

fn on_floor_at(dx: i32, dz: i32) -> BlockPos {
    at(CENTRE.0 + dx, -60, CENTRE.1 + dz)
}

// ---------------------------------------------------------------------------------------
// Spreading on flat ground
// ---------------------------------------------------------------------------------------

/// Specification 6: water on flat open ground spreads to a diamond, level d at
/// distance d up to 7 and nothing at 8.
#[test]
fn water_on_flat_ground_spreads_to_a_diamond_of_levels_one_to_seven() {
    let source = on_floor_at(0, 0);
    let (mut region, _) = sources_of(WATER, &[source], &[]);
    run(&mut region, 100);
    for dx in -10..=10_i32 {
        for dz in -10..=10_i32 {
            let distance = (dx.abs() + dz.abs()) as u8;
            let expected = (distance <= 7).then_some(distance);
            assert_eq!(
                level_at(&region, on_floor_at(dx, dz)),
                expected,
                "{dx}, {dz}"
            );
            assert_eq!(
                block_at(&region, at(CENTRE.0 + dx, -59, CENTRE.1 + dz)),
                Some(blocks::AIR),
                "nothing climbs at {dx}, {dz}"
            );
        }
    }
}

/// Specification 6: lava spreads to distance 3 at levels 2, 4 and 6.
#[test]
fn lava_on_flat_ground_spreads_three_blocks_at_levels_two_four_and_six() {
    let source = on_floor_at(0, 0);
    let (mut region, _) = sources_of(LAVA, &[source], &[]);
    run(&mut region, 1000);
    for dx in -6..=6_i32 {
        for dz in -6..=6_i32 {
            let distance = (dx.abs() + dz.abs()) as u8;
            let expected = (distance <= 3).then_some(distance * 2);
            assert_eq!(
                level_at(&region, on_floor_at(dx, dz)),
                expected,
                "{dx}, {dz}"
            );
        }
    }
}

/// Specification 3: water spreads five ticks after its source's tick, one block at a
/// time.
#[test]
fn water_reaches_each_further_block_five_ticks_after_the_last() {
    let source = on_floor_at(0, 0);
    let (mut region, _) = sources_of(WATER, &[source], &[]);
    // The source spread in the tick the world loaded in; each flowing block spreads at
    // its own tick, five ticks after it was set.
    assert_eq!(level_at(&region, on_floor_at(1, 0)), Some(1));
    run(&mut region, 4);
    assert_eq!(level_at(&region, on_floor_at(2, 0)), None);
    run(&mut region, 1);
    assert_eq!(level_at(&region, on_floor_at(2, 0)), Some(2));
    run(&mut region, 4);
    assert_eq!(level_at(&region, on_floor_at(3, 0)), None);
    run(&mut region, 1);
    assert_eq!(level_at(&region, on_floor_at(3, 0)), Some(3));
}

/// Specification 3: lava spreads thirty ticks after its source's tick.
#[test]
fn lava_reaches_the_second_block_thirty_ticks_after_the_first() {
    let source = on_floor_at(0, 0);
    let (mut region, _) = sources_of(LAVA, &[source], &[]);
    assert_eq!(level_at(&region, on_floor_at(1, 0)), Some(2));
    run(&mut region, 29);
    assert_eq!(level_at(&region, on_floor_at(2, 0)), None);
    run(&mut region, 1);
    assert_eq!(level_at(&region, on_floor_at(2, 0)), Some(4));
}

// ---------------------------------------------------------------------------------------
// Towards the way down
// ---------------------------------------------------------------------------------------

/// A hole through the floor at `(dx, dz)` from the centre, with stone below it to keep
/// what falls in.
fn hole(dx: i32, dz: i32) -> [(BlockPos, BlockState); 2] {
    let (x, z) = (CENTRE.0 + dx, CENTRE.1 + dz);
    [(at(x, -61, z), blocks::AIR), (at(x, -62, z), blocks::STONE)]
}

/// Specification 5: of a hole two blocks east and one four blocks west, both within
/// water's reach of four, the water goes only east.
#[test]
fn water_flows_only_towards_the_nearest_way_down() {
    let source = on_floor_at(0, 0);
    let blocks: Vec<_> = hole(2, 0).into_iter().chain(hole(-4, 0)).collect();
    let (mut region, _) = sources_of(WATER, &[source], &blocks);
    run(&mut region, 60);
    assert_eq!(level_at(&region, on_floor_at(1, 0)), Some(1));
    assert_eq!(level_at(&region, on_floor_at(2, 0)), Some(2));
    assert_eq!(level_at(&region, at(10, -61, 8)), Some(8), "it falls in");
    for (dx, dz) in [(-1, 0), (0, -1), (0, 1), (3, 0)] {
        assert_eq!(level_at(&region, on_floor_at(dx, dz)), None, "{dx}, {dz}");
    }
    assert_eq!(
        level_at(&region, at(4, -61, 8)),
        None,
        "the far hole stays dry"
    );
}

/// Specification 5: water goes to every side that ties for the nearest way down.
#[test]
fn water_flows_to_both_of_two_equally_near_ways_down() {
    let source = on_floor_at(0, 0);
    let blocks: Vec<_> = hole(2, 0).into_iter().chain(hole(-2, 0)).collect();
    let (mut region, _) = sources_of(WATER, &[source], &blocks);
    run(&mut region, 60);
    for dx in [-1, 1] {
        assert_eq!(level_at(&region, on_floor_at(dx, 0)), Some(1), "{dx}");
        assert_eq!(level_at(&region, on_floor_at(2 * dx, 0)), Some(2), "{dx}");
    }
    assert_eq!(level_at(&region, at(6, -61, 8)), Some(8));
    assert_eq!(level_at(&region, at(10, -61, 8)), Some(8));
    for dz in [-1, 1] {
        assert_eq!(level_at(&region, on_floor_at(0, dz)), None, "{dz}");
    }
}

/// Specification 5: a way down further than water looks (four blocks) does not steer
/// it; it spreads to every side.
#[test]
fn water_with_no_way_down_within_four_blocks_spreads_to_every_side() {
    let source = on_floor_at(0, 0);
    let (_, output) = sources_of(WATER, &[source], &hole(6, 0));
    let reported = block_changes(&output);
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        assert!(
            reported.contains(&RegionEvent::BlockChanged {
                position: on_floor_at(dx, dz),
                state: water(1),
                seen: true,
            }),
            "{dx}, {dz}: {reported:?}"
        );
    }
}

/// Specification 5: water looks four blocks on from the block beside it, so a way
/// down five blocks off steers it.
#[test]
fn water_is_steered_by_a_way_down_five_blocks_off() {
    let source = on_floor_at(0, 0);
    let (_, output) = sources_of(WATER, &[source], &hole(5, 0));
    assert_eq!(block_changes(&output), [changed(9, -60, 8, water(1))]);
}

/// Specification 5: lava looks two blocks on from the block beside it for a way down,
/// against water's four.
#[test]
fn lava_is_steered_by_a_way_down_three_blocks_off_and_not_by_one_four_off() {
    let source = on_floor_at(0, 0);
    let (_, output) = sources_of(LAVA, &[source], &hole(3, 0));
    let reported: Vec<BlockPos> = block_changes(&output)
        .into_iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged { position, .. } => Some(position),
            _ => None,
        })
        .collect();
    assert_eq!(reported, [on_floor_at(1, 0)]);

    let (_, output) = sources_of(LAVA, &[source], &hole(4, 0));
    let mut reported: Vec<BlockPos> = block_changes(&output)
        .into_iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged { position, .. } => Some(position),
            _ => None,
        })
        .collect();
    reported.sort_by_key(|position| (position.x, position.z));
    let mut every_side = vec![
        on_floor_at(1, 0),
        on_floor_at(-1, 0),
        on_floor_at(0, 1),
        on_floor_at(0, -1),
    ];
    every_side.sort_by_key(|position| (position.x, position.z));
    assert_eq!(reported, every_side);
}

// ---------------------------------------------------------------------------------------
// Falling
// ---------------------------------------------------------------------------------------

/// Specification 1 and 5: a source in the air falls in a column of level 8 that does
/// not spread on its way down, and spreads at the bottom as from amount 8: level d at
/// distance d. (The source is walled in on its four sides: a source over water spreads
/// sideways, which is not what this is about.)
#[test]
fn a_falling_column_is_level_eight_and_spreads_only_at_the_bottom() {
    let source = at(8, -55, 8);
    let walls = [
        (at(7, -55, 8), blocks::STONE),
        (at(9, -55, 8), blocks::STONE),
        (at(8, -55, 7), blocks::STONE),
        (at(8, -55, 9), blocks::STONE),
    ];
    let (mut region, _) = sources_of(WATER, &[source], &walls);
    run(&mut region, 120);
    assert_eq!(level_at(&region, source), Some(0));
    for y in -60..=-56 {
        assert_eq!(level_at(&region, at(8, y, 8)), Some(8), "{y}");
    }
    for y in -59..=-56 {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            assert_eq!(
                level_at(&region, at(8 + dx, y, 8 + dz)),
                None,
                "nothing beside the column at {y}"
            );
        }
    }
    for dx in -9..=9_i32 {
        for dz in -9..=9_i32 {
            let distance = (dx.abs() + dz.abs()) as u8;
            let expected = match distance {
                0 => Some(8),
                1..=7 => Some(distance),
                _ => None,
            };
            assert_eq!(
                level_at(&region, on_floor_at(dx, dz)),
                expected,
                "{dx}, {dz}"
            );
        }
    }
}

/// Specification 5: a source that goes down spreads sideways as well only when three
/// of its sides are sources of water.
#[test]
fn a_source_that_goes_down_spreads_sideways_only_beside_three_sources() {
    // The source at -59 stands over air at -60; the blocks beside it stand on stone.
    let source = at(8, -59, 8);
    let pillars = [at(7, -60, 8), at(9, -60, 8), at(8, -60, 7), at(8, -60, 9)];
    let open = at(8, -59, 9);
    let mut blocks: Vec<_> = pillars.iter().map(|p| (*p, blocks::STONE)).collect();
    blocks.push((source, blocks::WATER));

    let two = [
        (at(7, -59, 8), blocks::WATER),
        (at(9, -59, 8), blocks::WATER),
    ];
    let mut with_two = blocks.clone();
    with_two.extend(two);
    let (region, output) = world_with(&with_two, &[(source, fluid_tick(source, WATER))]);
    assert!(
        block_changes(&output).contains(&RegionEvent::BlockChanged {
            position: at(8, -60, 8),
            state: water(8),
            seen: true,
        }),
        "it goes down"
    );
    assert_eq!(level_at(&region, open), None);
    assert_eq!(level_at(&region, at(8, -59, 7)), None);

    blocks.extend(two);
    blocks.push((at(8, -59, 7), blocks::WATER));
    let (region, output) = world_with(&blocks, &[(source, fluid_tick(source, WATER))]);
    assert!(block_changes(&output).contains(&RegionEvent::BlockChanged {
        position: at(8, -60, 8),
        state: water(8),
        seen: true,
    }));
    assert_eq!(level_at(&region, open), Some(1));
}

// ---------------------------------------------------------------------------------------
// New sources
// ---------------------------------------------------------------------------------------

/// Specification 4: water between two sources on stone becomes a source.
#[test]
fn water_between_two_sources_on_solid_ground_becomes_a_source() {
    let (mut region, _) = sources_of(WATER, &[on_floor_at(-1, 0), on_floor_at(1, 0)], &[]);
    run(&mut region, 20);
    assert_eq!(level_at(&region, on_floor_at(0, 0)), Some(0));
}

/// Specification 4: water between two sources over a source of water becomes a source.
#[test]
fn water_between_two_sources_over_a_source_becomes_a_source() {
    let blocks = [
        (at(8, -61, 8), blocks::WATER),
        (at(8, -62, 8), blocks::STONE),
    ];
    let (mut region, _) = sources_of(WATER, &[on_floor_at(-1, 0), on_floor_at(1, 0)], &blocks);
    run(&mut region, 20);
    assert_eq!(level_at(&region, on_floor_at(0, 0)), Some(0));
    assert_eq!(level_at(&region, at(8, -61, 8)), Some(0));
}

/// Specification 4: water between two sources over air flows on and stays flowing.
#[test]
fn water_between_two_sources_over_air_does_not_become_a_source() {
    // The sources stand on pillars at -56 with air between them down to the floor.
    let (west, east) = (at(7, -55, 8), at(9, -55, 8));
    let blocks = [
        (at(7, -56, 8), blocks::STONE),
        (at(9, -56, 8), blocks::STONE),
    ];
    let (mut region, _) = sources_of(WATER, &[west, east], &blocks);
    run(&mut region, 60);
    assert_eq!(level_at(&region, at(8, -55, 8)), Some(1));
    assert_eq!(level_at(&region, at(8, -56, 8)), Some(8));
}

/// Specification 4: lava never becomes a source between two sources.
#[test]
fn lava_between_two_sources_on_solid_ground_does_not_become_a_source() {
    let (mut region, _) = sources_of(LAVA, &[on_floor_at(-1, 0), on_floor_at(1, 0)], &[]);
    run(&mut region, 400);
    assert_eq!(level_at(&region, on_floor_at(0, 0)), Some(2));
}

// ---------------------------------------------------------------------------------------
// Blocks in the way
// ---------------------------------------------------------------------------------------

/// Specification 8: flowing water never waterlogs a block.
#[test]
fn flowing_water_does_not_waterlog_a_slab_beside_it() {
    let slab = on_floor_at(2, 0);
    let dry = state("minecraft:stone_slab[type=bottom,waterlogged=false]");
    let (mut region, _) = sources_of(WATER, &[on_floor_at(0, 0)], &[(slab, dry)]);
    run(&mut region, 60);
    assert_eq!(level_at(&region, on_floor_at(1, 0)), Some(1));
    assert_eq!(block_at(&region, slab), Some(dry));
}

/// Specification 9: a waterloggable block between two still sources on solid ground
/// takes their water, as the source it would become.
#[test]
fn a_slab_between_two_sources_on_solid_ground_is_waterlogged() {
    let slab = on_floor_at(0, 0);
    let dry = state("minecraft:stone_slab[type=bottom,waterlogged=false]");
    let (mut region, _) = sources_of(
        WATER,
        &[on_floor_at(-1, 0), on_floor_at(1, 0)],
        &[(slab, dry)],
    );
    run(&mut region, 20);
    assert_eq!(
        block_at(&region, slab),
        Some(state("minecraft:stone_slab[type=bottom,waterlogged=true]"))
    );
}

/// Specification 9: the waterlogged block gets a water tick of five.
#[test]
fn a_block_waterlogged_by_spreading_water_gets_a_water_tick_of_five() {
    let slab = on_floor_at(0, 0);
    let dry = state("minecraft:stone_slab[type=bottom,waterlogged=false]");
    let (region, output) = sources_of(
        WATER,
        &[on_floor_at(-1, 0), on_floor_at(1, 0)],
        &[(slab, dry)],
    );
    assert!(
        block_changes(&output).contains(&RegionEvent::BlockChanged {
            position: slab,
            state: state("minecraft:stone_slab[type=bottom,waterlogged=true]"),
            seen: true,
        }),
        "the sources waterlog the slab at their first tick"
    );
    let saved = region
        .saved_chunk(ChunkPos::new(0, 0))
        .expect("the chunk is loaded");
    let ticks: Vec<_> = saved
        .ticks()
        .iter()
        .filter(|tick| tick.at == in_chunk(slab))
        .map(|tick| (tick.of, tick.delay))
        .collect();
    // Kept at the end of the tick it was scheduled in with one less than its delay.
    assert_eq!(ticks, [(Ticked::Fluid(WATER), 4)]);
}

/// Specification 9: a lit campfire or candle waterlogged so goes out.
#[test]
fn a_lit_campfire_or_candle_waterlogged_by_spreading_water_goes_out() {
    for (lit, out) in [
        (
            "minecraft:campfire[lit=true,waterlogged=false]",
            "minecraft:campfire[lit=false,waterlogged=true]",
        ),
        (
            "minecraft:candle[lit=true,waterlogged=false]",
            "minecraft:candle[lit=false,waterlogged=true]",
        ),
    ] {
        let middle = on_floor_at(0, 0);
        let (mut region, _) = sources_of(
            WATER,
            &[on_floor_at(-1, 0), on_floor_at(1, 0)],
            &[(middle, state(lit))],
        );
        run(&mut region, 20);
        assert_eq!(block_at(&region, middle), Some(state(out)), "{lit}");
    }
}

/// Specification 8: seagrass and kelp take no water but are not washed away.
#[test]
fn water_does_not_wash_away_seagrass_or_kelp() {
    for plant in [blocks::SEAGRASS, blocks::KELP] {
        let at_plant = on_floor_at(2, 0);
        let (mut region, _) = sources_of(WATER, &[on_floor_at(0, 0)], &[(at_plant, plant)]);
        run(&mut region, 60);
        assert_eq!(block_at(&region, at_plant), Some(plant), "{plant:?}");
    }
}

/// Specification 8: what is in `washed_away_by_fluids` gives way to water.
#[test]
fn water_washes_away_a_torch_and_short_grass() {
    let (torch, grass) = (on_floor_at(1, 0), on_floor_at(0, 1));
    let (mut region, _) = sources_of(
        WATER,
        &[on_floor_at(0, 0)],
        &[
            (torch, blocks::TORCH),
            (grass, blocks::SHORT_GRASS),
            (at(8, -61, 9), blocks::DIRT),
        ],
    );
    run(&mut region, 5);
    assert_eq!(level_at(&region, torch), Some(1));
    assert_eq!(level_at(&region, grass), Some(1));
}

/// Specification 8: a full block is a wall; water does not flow through it, and a
/// stone wall right across the chunk keeps it out of the far side.
#[test]
fn water_does_not_pass_through_a_full_block() {
    let wall: Vec<_> = (-16..32).map(|z| (at(9, -60, z), blocks::STONE)).collect();
    let (mut region, _) = sources_of(WATER, &[on_floor_at(0, 0)], &wall);
    run(&mut region, 60);
    assert_eq!(block_at(&region, on_floor_at(1, 0)), Some(blocks::STONE));
    for dz in -7..=7 {
        assert_eq!(level_at(&region, on_floor_at(2, dz)), None, "{dz}");
    }
}

/// Specification 8: a bottom slab lets water pass over its top: water flows over a
/// floor of bottom slabs as over stone, a diamond above the slabs, and leaves them dry
/// (above them is no way down, since a slab takes no flowing water).
#[test]
fn water_flows_over_bottom_slabs_as_over_solid_ground_and_leaves_them_dry() {
    let dry = state("minecraft:stone_slab[type=bottom,waterlogged=false]");
    let slabs: Vec<_> = (-16..32)
        .flat_map(|x| (-16..32).map(move |z| (at(x, -60, z), dry)))
        .collect();
    let source = at(8, -59, 8);
    let (mut region, _) = sources_of(WATER, &[source], &slabs);
    run(&mut region, 100);
    for dx in -9..=9_i32 {
        for dz in -9..=9_i32 {
            let distance = (dx.abs() + dz.abs()) as u8;
            let expected = (distance <= 7).then_some(distance);
            let above = at(8 + dx, -59, 8 + dz);
            assert_eq!(level_at(&region, above), expected, "{dx}, {dz}");
            assert_eq!(block_at(&region, at(8 + dx, -60, 8 + dz)), Some(dry));
        }
    }
}

/// Specification 8: water in a waterlogged bottom slab flows out of its half-high sides
/// and falls into the slab from above.
#[test]
fn water_flows_sideways_out_of_a_waterlogged_bottom_slab() {
    let slab = on_floor_at(0, 0);
    let wet = state("minecraft:stone_slab[type=bottom,waterlogged=true]");
    let (region, _) = world_with(&[(slab, wet)], &[(slab, fluid_tick(slab, WATER))]);
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        assert_eq!(
            level_at(&region, on_floor_at(dx, dz)),
            Some(1),
            "{dx}, {dz}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// The tick of the wrong fluid
// ---------------------------------------------------------------------------------------

/// Specification 2: a tick of still water at flowing water does nothing; one of
/// flowing water there recomputes it, and flowing water with nothing feeding it dries.
#[test]
fn a_tick_for_another_fluid_than_the_block_holds_does_nothing() {
    let flowing = on_floor_at(0, 0);
    let (mut region, output) = world_with(
        &[(flowing, water(3))],
        &[(flowing, fluid_tick(flowing, WATER))],
    );
    assert_eq!(block_changes(&output), []);
    run(&mut region, 10);
    assert_eq!(block_at(&region, flowing), Some(water(3)));

    let (_, output) = world_with(
        &[(flowing, water(3))],
        &[(flowing, fluid_tick(flowing, FLOWING_WATER))],
    );
    assert_eq!(
        block_changes(&output),
        [changed(8, -60, 8, blocks::AIR)],
        "the right fluid's tick dries it"
    );
}

/// Specification 2: a tick of lava at water does nothing.
#[test]
fn a_tick_of_lava_at_a_water_source_does_nothing() {
    let source = on_floor_at(0, 0);
    let (mut region, output) = world_with(
        &[(source, blocks::WATER)],
        &[(source, fluid_tick(source, LAVA))],
    );
    assert_eq!(block_changes(&output), []);
    assert_eq!(run(&mut region, 10), []);
}

// ---------------------------------------------------------------------------------------
// Placing a liquid
// ---------------------------------------------------------------------------------------

/// Specification 13: water set in place spreads at its tick five ticks on.
#[test]
fn placed_water_spreads_five_ticks_later() {
    let (mut region, _) = world_with(&[], &[]);
    set(&mut region, on_floor_at(0, 0), blocks::WATER);
    run(&mut region, 4);
    assert_eq!(level_at(&region, on_floor_at(1, 0)), None);
    run(&mut region, 1);
    assert_eq!(level_at(&region, on_floor_at(1, 0)), Some(1));
}

/// Specification 11 and 13: lava set in place with no water about spreads at its tick
/// thirty ticks on.
#[test]
fn placed_lava_spreads_thirty_ticks_later() {
    let (mut region, _) = world_with(&[], &[]);
    let output = set(&mut region, on_floor_at(0, 0), blocks::LAVA);
    assert_eq!(fizzes(&output.events), []);
    run(&mut region, 29);
    assert_eq!(level_at(&region, on_floor_at(1, 0)), None);
    run(&mut region, 1);
    assert_eq!(level_at(&region, on_floor_at(1, 0)), Some(2));
}

/// A water bucket aimed at a dry bottom slab south of player 1.
fn bucket_into_a_slab() -> (Region, BlockPos) {
    let slab = at(0, -60, 1);
    let dry = state("minecraft:stone_slab[type=bottom,waterlogged=false]");
    let (mut region, _) = world_with(&[(slab, dry)], &[]);
    let bucket = ItemStack {
        item: items::WATER_BUCKET,
        count: 1,
    };
    // Inputs are numbered as they are made, and applied only in that order.
    let inputs = vec![
        set_slot(1, 0, Some(bucket)),
        select(1, 0),
        numbered(
            1,
            PlayerInput::UseItem {
                rotation: (0.0, 60.0),
                sneaking: false,
                sequence: 1,
            },
        ),
    ];
    let output = checked(&mut region, &moves(inputs));
    assert_eq!(
        block_changes(&output),
        [changed(
            0,
            -60,
            1,
            state("minecraft:stone_slab[type=bottom,waterlogged=true]")
        )]
    );
    (region, slab)
}

/// Specification 13: a bucket that waterlogs a block schedules a water tick of five,
/// counted from the tick before the bucket's, as the game handles a player's packet
/// between ticks.
#[test]
fn a_bucket_that_waterlogs_a_slab_gives_it_a_water_tick_of_five() {
    let (mut region, slab) = bucket_into_a_slab();
    let east = at(slab.x + 1, slab.y, slab.z);
    run(&mut region, 3);
    assert_eq!(level_at(&region, east), None);
    run(&mut region, 1);
    assert_eq!(level_at(&region, east), Some(1));
}

// ---------------------------------------------------------------------------------------
// Lava meeting water
// ---------------------------------------------------------------------------------------

/// Specification 11: lava placed with still water above it or on any of its four sides
/// becomes obsidian, with a hiss there.
#[test]
fn a_lava_source_placed_against_water_above_or_beside_becomes_obsidian() {
    let lava_at = on_floor_at(0, 0);
    for water_at in [
        at(8, -59, 8),
        on_floor_at(1, 0),
        on_floor_at(-1, 0),
        on_floor_at(0, 1),
        on_floor_at(0, -1),
    ] {
        let (mut region, _) = world_with(&[(water_at, blocks::WATER)], &[]);
        let output = set(&mut region, lava_at, blocks::LAVA);
        assert_eq!(
            block_at(&region, lava_at),
            Some(blocks::OBSIDIAN),
            "{water_at:?}"
        );
        assert_eq!(fizzes(&output.events), [lava_at], "{water_at:?}");
    }
}

/// Specification 11: flowing water beside counts as water as well.
#[test]
fn a_lava_source_placed_beside_flowing_water_becomes_obsidian() {
    let lava_at = on_floor_at(0, 0);
    let (mut region, _) = world_with(&[(on_floor_at(1, 0), water(3))], &[]);
    set(&mut region, lava_at, blocks::LAVA);
    assert_eq!(block_at(&region, lava_at), Some(blocks::OBSIDIAN));
}

/// Specification 11: flowing lava placed beside water becomes cobblestone.
#[test]
fn flowing_lava_placed_beside_water_becomes_cobblestone() {
    let lava_at = on_floor_at(0, 0);
    let (mut region, _) = world_with(&[(on_floor_at(0, 1), blocks::WATER)], &[]);
    let output = set(&mut region, lava_at, lava(2));
    assert_eq!(block_at(&region, lava_at), Some(blocks::COBBLESTONE));
    assert_eq!(fizzes(&output.events), [lava_at]);
}

/// Specification 11: water below lava does not harden it when it is placed; the lava
/// waits for its tick, and then flows down into the water and turns it to stone with a
/// hiss there (specification 10).
#[test]
fn lava_over_water_stays_lava_and_at_its_tick_turns_the_water_below_to_stone() {
    let (lava_at, water_at) = (at(8, -59, 8), on_floor_at(0, 0));
    let (mut region, _) = world_with(&[(water_at, blocks::WATER)], &[]);
    let output = set(&mut region, lava_at, blocks::LAVA);
    assert_eq!(block_at(&region, lava_at), Some(blocks::LAVA));
    assert_eq!(fizzes(&output.events), []);
    let events = run(&mut region, 29);
    assert_eq!(fizzes(&events), []);
    assert_eq!(
        block_at(&region, water_at).map(|b| is_class(b, BlockClass::LiquidBlock)),
        Some(true)
    );
    let events = run(&mut region, 1);
    assert_eq!(block_at(&region, water_at), Some(blocks::STONE));
    assert_eq!(fizzes(&events), [water_at]);
    assert_eq!(block_at(&region, lava_at), Some(blocks::LAVA));
}

/// Specification 10: flowing water below falling lava turns to stone as well.
#[test]
fn lava_flowing_down_into_flowing_water_turns_it_to_stone() {
    let (lava_at, water_at) = (at(8, -59, 8), on_floor_at(0, 0));
    let (region, output) = world_with(
        &[(lava_at, blocks::LAVA), (water_at, water(7))],
        &[(lava_at, fluid_tick(lava_at, LAVA))],
    );
    assert_eq!(block_at(&region, water_at), Some(blocks::STONE));
    assert_eq!(fizzes(&output.events), [water_at]);
}

/// Specification 11: lava on soul soil beside blue ice becomes basalt, with a hiss.
#[test]
fn lava_on_soul_soil_beside_blue_ice_becomes_basalt() {
    let lava_at = on_floor_at(0, 0);
    let (mut region, _) = world_with(
        &[
            (at(8, -61, 8), blocks::SOUL_SOIL),
            (on_floor_at(1, 0), blocks::BLUE_ICE),
        ],
        &[],
    );
    let output = set(&mut region, lava_at, blocks::LAVA);
    assert_eq!(
        block_at(&region, lava_at).map(|b| b.block_id()),
        Some(blocks::BASALT.block_id())
    );
    assert_eq!(fizzes(&output.events), [lava_at]);
}

/// Specification 11: blue ice beside lava that is not on soul soil does nothing, nor
/// does soul soil with no blue ice.
#[test]
fn lava_beside_blue_ice_off_soul_soil_or_on_soul_soil_alone_stays_lava() {
    for around in [
        vec![(on_floor_at(1, 0), blocks::BLUE_ICE)],
        vec![(at(8, -61, 8), blocks::SOUL_SOIL)],
    ] {
        let lava_at = on_floor_at(0, 0);
        let (mut region, _) = world_with(&around, &[]);
        let output = set(&mut region, lava_at, blocks::LAVA);
        assert_eq!(block_at(&region, lava_at), Some(blocks::LAVA), "{around:?}");
        assert_eq!(fizzes(&output.events), []);
    }
}

/// Specification 11: lava told that water came beside it hardens then.
#[test]
fn a_lava_source_told_of_water_beside_it_becomes_obsidian() {
    let lava_at = on_floor_at(0, 0);
    let (mut region, _) = world_with(&[(lava_at, blocks::LAVA)], &[]);
    let output = set(&mut region, on_floor_at(-1, 0), blocks::WATER);
    assert_eq!(block_at(&region, lava_at), Some(blocks::OBSIDIAN));
    assert_eq!(fizzes(&output.events), [lava_at]);
}

/// Specification 11: flowing lava that spreads beside still water becomes cobblestone
/// at once; the water, which did not tick, stays.
#[test]
fn lava_that_flows_beside_water_becomes_cobblestone() {
    let water_at = on_floor_at(3, 0);
    let (mut region, _) = sources_of(LAVA, &[on_floor_at(0, 0)], &[(water_at, blocks::WATER)]);
    // Until the cobblestone is there: the water, told of it, flows from then on.
    let mut events = Vec::new();
    for _ in 0..300 {
        if block_at(&region, on_floor_at(2, 0)) == Some(blocks::COBBLESTONE) {
            break;
        }
        events.extend(idle_tick(&mut region).events);
    }
    assert_eq!(level_at(&region, on_floor_at(1, 0)), Some(2));
    assert_eq!(
        block_at(&region, on_floor_at(2, 0)),
        Some(blocks::COBBLESTONE)
    );
    assert!(fizzes(&events).contains(&on_floor_at(2, 0)));
    assert_eq!(block_at(&region, water_at), Some(blocks::WATER));
}

/// Specification 10: lava hisses where it takes the place of a block other than air,
/// and not where it fills air.
#[test]
fn lava_hisses_where_it_washes_a_block_away_and_not_into_air() {
    let grass = on_floor_at(1, 0);
    let (_, output) = sources_of(
        LAVA,
        &[on_floor_at(0, 0)],
        &[(grass, blocks::SHORT_GRASS), (at(9, -61, 8), blocks::DIRT)],
    );
    assert_eq!(level_at_output(&output, grass), Some(lava(2)));
    assert_eq!(fizzes(&output.events), [grass]);
}

/// What the tick that `output` reports set at `position`, last.
fn level_at_output(output: &TickOutput, position: BlockPos) -> Option<BlockState> {
    output.events.iter().rev().find_map(|event| match event {
        RegionEvent::BlockChanged {
            position: p, state, ..
        } if *p == position => Some(*state),
        _ => None,
    })
}

/// Specification 7: water does not give way to lava flowing sideways into it; the lava
/// beside it hardens instead, and the water stays.
#[test]
fn flowing_water_is_not_replaced_by_lava_from_the_side() {
    let water_at = on_floor_at(1, 0);
    let (region, output) = sources_of(LAVA, &[on_floor_at(0, 0)], &[(water_at, water(7))]);
    assert_eq!(block_at(&region, water_at), Some(water(7)));
    assert_eq!(level_at_output(&output, water_at), None);
}

// ---------------------------------------------------------------------------------------
// Concrete powder
// ---------------------------------------------------------------------------------------

const POWDER: ItemStack = ItemStack {
    item: items::WHITE_CONCRETE_POWDER,
    count: 1,
};

/// Player 1 places white concrete powder on the top of the block at (2, -61, 0), so
/// at (2, -60, 0), in a world with `blocks`; returns what was placed there.
fn powder_placed_with(blocks: &[(BlockPos, BlockState)]) -> Option<BlockState> {
    let (mut region, _) = world_with(blocks, &[]);
    let output = checked(
        &mut region,
        &moves(vec![
            set_slot(1, 4, Some(POWDER)),
            select(1, 4),
            place(1, 2, -61, 0, Face::Top),
        ]),
    );
    level_at_output(&output, at(2, -60, 0))
}

/// Specification 12: powder placed beside water is concrete at once.
#[test]
fn concrete_powder_placed_beside_water_is_concrete() {
    let placed = powder_placed_with(&[(at(3, -60, 0), blocks::WATER)]);
    assert_eq!(placed, Some(blocks::WHITE_CONCRETE));
}

/// Specification 12: powder placed with no water about stays powder.
#[test]
fn concrete_powder_placed_away_from_water_stays_powder() {
    let placed = powder_placed_with(&[(at(5, -60, 0), blocks::WATER)]);
    assert_eq!(placed, Some(blocks::WHITE_CONCRETE_POWDER));
}

/// Specification 12: powder placed into water is concrete at once.
#[test]
fn concrete_powder_placed_into_water_is_concrete() {
    let placed = powder_placed_with(&[(at(2, -60, 0), blocks::WATER)]);
    assert_eq!(placed, Some(blocks::WHITE_CONCRETE));
}

/// Specification 12: water whose touching face is sturdy, a waterlogged top slab's
/// top, does not harden the powder placed on it; a waterlogged bottom slab's top,
/// which is not sturdy, does.
#[test]
fn concrete_powder_on_a_waterlogged_slab_hardens_only_where_the_face_is_not_sturdy() {
    let top = state("minecraft:stone_slab[type=top,waterlogged=true]");
    let placed = powder_placed_with(&[(at(2, -61, 0), top)]);
    assert_eq!(placed, Some(blocks::WHITE_CONCRETE_POWDER));

    let bottom = state("minecraft:stone_slab[type=bottom,waterlogged=true]");
    let placed = powder_placed_with(&[(at(2, -61, 0), bottom)]);
    assert_eq!(placed, Some(blocks::WHITE_CONCRETE));
}

/// Specification 12: powder told that water came beside it hardens.
#[test]
fn concrete_powder_told_of_water_beside_it_hardens() {
    let powder = on_floor_at(0, 0);
    let (mut region, _) = world_with(&[(powder, blocks::WHITE_CONCRETE_POWDER)], &[]);
    set(&mut region, on_floor_at(0, 1), blocks::WATER);
    assert_eq!(block_at(&region, powder), Some(blocks::WHITE_CONCRETE));
}

/// Specification 12: falling powder stops in the first water below it and hardens
/// there; it does not sink to the bottom of the pool as sand does.
#[test]
fn falling_concrete_powder_stops_and_hardens_in_the_first_water_it_reaches() {
    let powder = at(8, -55, 8);
    let (surface, bottom) = (at(8, -59, 8), at(8, -60, 8));
    let (region, _) = world_with(
        &[
            (powder, blocks::WHITE_CONCRETE_POWDER),
            (surface, blocks::WATER),
            (bottom, blocks::WATER),
        ],
        &[(powder, block_tick(powder, blocks::WHITE_CONCRETE_POWDER))],
    );
    assert_eq!(block_at(&region, powder), Some(blocks::AIR));
    assert_eq!(block_at(&region, surface), Some(blocks::WHITE_CONCRETE));
    assert_eq!(block_at(&region, bottom), Some(blocks::WATER));

    // Sand in the same place sinks through both.
    let (region, _) = world_with(
        &[
            (powder, blocks::SAND),
            (surface, blocks::WATER),
            (bottom, blocks::WATER),
        ],
        &[(powder, block_tick(powder, blocks::SAND))],
    );
    assert_eq!(block_at(&region, bottom), Some(blocks::SAND));
}

/// Specification 4: flowing water with no source left recomputes to less and less,
/// and dries up altogether.
#[test]
fn water_dries_up_when_its_source_is_taken_away() {
    let source = on_floor_at(0, 0);
    let (mut region, _) = sources_of(WATER, &[source], &[]);
    run(&mut region, 100);
    assert_eq!(level_at(&region, on_floor_at(3, 0)), Some(3));
    set(&mut region, source, blocks::AIR);
    run(&mut region, 200);
    for dx in -9..=9_i32 {
        for dz in -9..=9_i32 {
            assert_eq!(
                block_at(&region, on_floor_at(dx, dz)),
                Some(blocks::AIR),
                "{dx}, {dz}"
            );
        }
    }
}

/// Specification 7: water flowing sideways takes the place of lava at least 4/9 high
/// (amount 4 or more), and not of lower lava.
#[test]
fn water_takes_the_place_of_lava_only_where_it_is_at_least_four_ninths_high() {
    let (source, beside) = (on_floor_at(0, 0), on_floor_at(1, 0));
    let (region, output) = sources_of(WATER, &[source], &[(beside, lava(4))]);
    assert_eq!(level_at_output(&output, beside), Some(water(1)));
    assert_eq!(block_at(&region, beside), Some(water(1)));
    assert_eq!(fizzes(&output.events), []);

    let (region, output) = sources_of(WATER, &[source], &[(beside, lava(5))]);
    assert_eq!(level_at_output(&output, beside), None);
    assert_eq!(block_at(&region, beside), Some(lava(5)));
}

/// A water bucket emptied is heard at the block by everyone near but its user, as the
/// game's `BucketItem.playEmptySound` plays it (ADR-0026, section 8, step R6).
#[test]
fn a_bucket_emptied_is_heard_at_its_block_by_all_but_its_user() {
    let slab = at(0, -60, 1);
    let dry = state("minecraft:stone_slab[type=bottom,waterlogged=false]");
    let (mut region, _) = world_with(&[(slab, dry)], &[]);
    let bucket = ItemStack {
        item: items::WATER_BUCKET,
        count: 1,
    };
    let inputs = vec![
        set_slot(1, 0, Some(bucket)),
        select(1, 0),
        numbered(
            1,
            PlayerInput::UseItem {
                rotation: (0.0, 60.0),
                sneaking: false,
                sequence: 1,
            },
        ),
    ];
    let output = checked(&mut region, &moves(inputs));
    let heard: Vec<(i32, Vec3, Option<PlayerId>)> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                position,
                except,
                ..
            } => Some((*sound, *position, *except)),
            _ => None,
        })
        .collect();
    assert_eq!(
        heard,
        [(
            clustine_data::sounds::ITEM_BUCKET_EMPTY,
            Vec3::new(0.5, -59.5, 1.5),
            Some(player(1))
        )]
    );
}
