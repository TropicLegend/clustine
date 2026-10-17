//! Tests of torches, repeaters, comparators, observers and target blocks (step R4 of
//! `docs/adr/0026-redstone.md`), from the game's numbers as the jar's classes give
//! them: a torch's two ticks and its burnout, a repeater's delays, locking and
//! priorities, a comparator's modes, inputs and durable output, an observer's pulse
//! and a clock of two.
//!
//! The regions are [`ticking_with`]'s: the chunks within two of chunk (0, 0) loaded on
//! a stone floor at y = -61, the nine within one ticking, and player 1 at the spawn.

use clustine_blocks::signal;
use clustine_data::Face;
use clustine_world::Ticked;

use super::redstone::{block_at, click, holding, lever, sounds, state, ticking_with};
use super::*;
use crate::region::updates::relative;

fn lit(region: &Region, pos: BlockPos) -> bool {
    block_at(region, pos).and_then(|block| block.property("lit")) == Some("true")
}

fn powered(region: &Region, pos: BlockPos) -> bool {
    block_at(region, pos).and_then(|block| block.property("powered")) == Some("true")
}

fn wire_power(region: &Region, pos: BlockPos) -> u8 {
    signal::wire_power(block_at(region, pos).expect("loaded"))
}

/// A lever on the east side of the block at `pos`, which it powers directly.
fn wall_lever(on: bool) -> BlockState {
    state(&format!(
        "minecraft:lever[face=wall,facing=east,powered={on}]"
    ))
}

fn torch(lit: bool) -> BlockState {
    state(&format!("minecraft:redstone_torch[lit={lit}]"))
}

fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } => Some((*kind, *position, *data)),
            _ => None,
        })
        .collect()
}

/// The blocks a tick changed, in the order it changed them.
fn changed(output: &TickOutput) -> Vec<BlockPos> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged { position, .. } => Some(*position),
            _ => None,
        })
        .collect()
}

const SUPPORT: BlockPos = BlockPos::new(2, -60, 2);
const TORCH: BlockPos = BlockPos::new(2, -59, 2);
const SUPPORT_LEVER: BlockPos = BlockPos::new(3, -60, 2);

/// A torch on a block powered by a lever on its side goes out two ticks after the
/// lever is pulled, and is lit again two ticks after it is pulled back; while lit it
/// powers the block above it, which lights a lamp beside that block.
#[test]
fn a_torch_goes_out_two_ticks_after_its_block_is_powered_and_comes_back_two_after() {
    let above = relative(TORCH, Face::Up);
    let lamp = relative(above, Face::North);
    let mut region = ticking_with(&[
        (SUPPORT, blocks::STONE),
        (SUPPORT_LEVER, wall_lever(false)),
        (TORCH, torch(true)),
        (above, blocks::STONE),
        (lamp, blocks::REDSTONE_LAMP),
    ]);
    // The blocks were set without updates: a block set beside the lamp tells it.
    idle(&mut region);
    let mut output = TickOutput::default();
    region.set_block(relative(lamp, Face::North), blocks::STONE, &mut output);
    assert!(lit(&region, lamp), "the block above the torch is powered");

    region.tick(&click(SUPPORT_LEVER));
    assert!(lit(&region, TORCH));
    idle(&mut region);
    assert!(lit(&region, TORCH), "one tick on");
    idle(&mut region);
    assert_eq!(block_at(&region, TORCH), Some(torch(false)), "two ticks on");

    region.tick(&click(SUPPORT_LEVER));
    idle(&mut region);
    assert_eq!(block_at(&region, TORCH), Some(torch(false)));
    idle(&mut region);
    assert_eq!(block_at(&region, TORCH), Some(torch(true)));
}

/// A redstone torch placed on a powered block comes out lit, as the game places it,
/// and goes out two ticks on, told of itself as it tells its neighbours'
/// neighbours.
#[test]
fn a_torch_placed_on_a_powered_block_comes_out_lit_and_goes_out_two_ticks_on() {
    let mut region = ticking_with(&[(SUPPORT, blocks::STONE), (SUPPORT_LEVER, wall_lever(true))]);
    region.tick(&holding(items::REDSTONE_TORCH));
    region.tick(&click(SUPPORT));
    assert_eq!(block_at(&region, TORCH), Some(torch(true)));
    idle(&mut region);
    assert_eq!(block_at(&region, TORCH), Some(torch(true)));
    idle(&mut region);
    assert_eq!(block_at(&region, TORCH), Some(torch(false)));
}

/// The clock of one torch: a torch on the side of a block, a block above the torch,
/// which it powers, and wire on the first block beside the second. The wire powers the
/// torch's block, and the torch goes out two ticks after it lights, every four ticks.
const CLOCK_BLOCK: BlockPos = BlockPos::new(4, -60, 4);
const CLOCK_TORCH: BlockPos = BlockPos::new(5, -60, 4);
const CLOCK_ABOVE: BlockPos = BlockPos::new(5, -59, 4);
const CLOCK_WIRE: BlockPos = BlockPos::new(4, -59, 4);

fn wall_torch(lit: bool) -> BlockState {
    state(&format!(
        "minecraft:redstone_wall_torch[facing=east,lit={lit}]"
    ))
}

/// The clock, started by its upper block set where it was air.
fn torch_clock() -> Region {
    let mut region = ticking_with(&[
        (CLOCK_BLOCK, blocks::STONE),
        (CLOCK_TORCH, wall_torch(true)),
        (CLOCK_WIRE, blocks::REDSTONE_WIRE),
    ]);
    let mut output = TickOutput::default();
    region.set_block(CLOCK_ABOVE, blocks::STONE, &mut output);
    assert_eq!(wire_power(&region, CLOCK_WIRE), 15);
    region
}

/// The torch of the clock goes out every four ticks; the eighth time within 60 ticks
/// it burns out, with the game's level event 1502 at the torch (smoke and the hiss,
/// which the client plays), and stays out. The tick of 160 it schedules then is not
/// taken, as one of 2 is pending from the update its going out made, so the clock
/// stays out until something tells the torch of a neighbour 60 ticks after its last
/// going out, as in the game.
#[test]
fn a_torch_clock_burns_out_the_eighth_time_it_goes_out_within_sixty_ticks() {
    let mut region = torch_clock();
    let mut out = Vec::new();
    let mut burnt = Vec::new();
    for tick in 1..=80u64 {
        let before = lit(&region, CLOCK_TORCH);
        let output = idle(&mut region);
        if before && !lit(&region, CLOCK_TORCH) {
            out.push(tick);
        }
        if !level_events(&output).is_empty() {
            assert_eq!(level_events(&output), [(1502, CLOCK_TORCH, 0)], "at {tick}");
            burnt.push(tick);
        }
    }
    assert_eq!(out, [2, 6, 10, 14, 18, 22, 26, 30], "every four ticks");
    assert_eq!(burnt, [30], "the eighth");
    assert!(!lit(&region, CLOCK_TORCH));
    assert_eq!(region.torch_toggles().len(), 8);

    // Sixty ticks after the last going out, a neighbour's change lights it again two
    // ticks on, and the clock runs.
    for _ in 81..=95 {
        idle(&mut region);
    }
    let mut output = TickOutput::default();
    let beside = relative(CLOCK_TORCH, Face::South);
    region.set_block(beside, blocks::STONE, &mut output);
    idle(&mut region);
    idle(&mut region);
    assert!(lit(&region, CLOCK_TORCH));
}

/// A torch put out by a lever eight times within 60 ticks burns out, and with the lever
/// let go stays out until the tick of 160 it scheduled, which lights it.
#[test]
fn a_burnt_out_torch_is_lit_again_by_its_own_tick_160_ticks_on() {
    let mut region = ticking_with(&[
        (SUPPORT, blocks::STONE),
        (SUPPORT_LEVER, wall_lever(false)),
        (TORCH, torch(true)),
    ]);
    // On, and off, every four ticks: the torch goes out two ticks after each pull on.
    let mut burnt_at = None;
    for pull in 0..8 {
        region.tick(&click(SUPPORT_LEVER));
        idle(&mut region);
        let output = idle(&mut region);
        assert!(!lit(&region, TORCH), "pull {pull}");
        if !level_events(&output).is_empty() {
            assert_eq!(level_events(&output), [(1502, TORCH, 0)]);
            burnt_at = Some(region.tick);
        }
        region.tick(&click(SUPPORT_LEVER));
        idle(&mut region);
        idle(&mut region);
        assert_eq!(lit(&region, TORCH), pull < 7, "pull {pull}");
    }
    let burnt_at = burnt_at.expect("it burnt out");
    while region.tick < burnt_at + 159 {
        idle(&mut region);
        assert!(!lit(&region, TORCH), "at {}", region.tick);
    }
    idle(&mut region);
    assert!(lit(&region, TORCH), "160 ticks on");
}

/// The burnout list is not durable: a region restored from its state and its saved
/// chunks has none, so the burnt-out torch of the clock lights again at once when told
/// of a neighbour, where the region that ran on keeps it out.
#[test]
fn the_burnout_list_is_dropped_at_a_restore() {
    let mut region = torch_clock();
    for _ in 0..32 {
        idle(&mut region);
    }
    assert!(!lit(&region, CLOCK_TORCH));
    assert_eq!(region.torch_toggles().len(), 8);

    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = around
        .iter()
        .map(|position| (*position, region.saved_chunk(*position).expect("loaded")))
        .collect();
    let ticketed: Vec<(ChunkPos, Ticket)> = around
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let mut restored = Region::restore_loaded(
        region.config.clone(),
        region.state(),
        Holdings {
            pinned: Vec::new(),
            held: around.clone(),
            lent: Vec::new(),
            lent_away: Vec::new(),
        },
        &ticketed,
        chunks,
        Vec::new(),
    );
    assert!(restored.torch_toggles().is_empty());

    let beside = relative(CLOCK_TORCH, Face::South);
    for region in [&mut region, &mut restored] {
        // A restored region's chunks tick from its first tick.
        idle(region);
        let mut output = TickOutput::default();
        region.set_block(beside, blocks::STONE, &mut output);
        idle(region);
        idle(region);
    }
    assert!(!lit(&region, CLOCK_TORCH), "still burnt out");
    assert!(lit(&restored, CLOCK_TORCH), "lit again");
}

const LEVER: BlockPos = BlockPos::new(0, -60, 2);

fn repeater(facing: &str, delay: u8, powered: bool) -> BlockState {
    state(&format!(
        "minecraft:repeater[facing={facing},delay={delay},powered={powered},locked=false]"
    ))
}

/// A click on a repeater takes it to its next delay, 1 to 4 and back to 1, with no
/// sound; power passes through it two ticks late for each step of its delay.
#[test]
fn a_repeaters_delay_is_set_by_clicks_and_passes_power_two_ticks_late_a_step() {
    let at = relative(LEVER, Face::East);
    let lamp = relative(at, Face::East);
    for delay in 1..=4u8 {
        let mut region = ticking_with(&[
            (LEVER, lever(false)),
            (at, repeater("west", 1, false)),
            (lamp, blocks::REDSTONE_LAMP),
        ]);
        for _ in 1..delay {
            let output = region.tick(&click(at));
            assert_eq!(sounds(&output), []);
        }
        assert_eq!(
            block_at(&region, at).and_then(|block| block.property("delay")),
            Some(delay.to_string().as_str())
        );
        region.tick(&click(LEVER));
        for late in 1..2 * delay {
            idle(&mut region);
            assert!(!lit(&region, lamp), "delay {delay}, {late} ticks on");
        }
        idle(&mut region);
        assert!(powered(&region, at), "delay {delay}");
        assert!(lit(&region, lamp), "delay {delay}");
    }
    let mut region = ticking_with(&[(at, repeater("west", 4, false))]);
    region.tick(&click(at));
    assert_eq!(block_at(&region, at), Some(repeater("west", 1, false)));
}

/// A repeater placed facing a powered lever behind it schedules its tick of 1
/// (`DiodeBlock.setPlacedBy`) and is powered the tick after.
#[test]
fn a_repeater_placed_with_power_behind_it_is_powered_the_tick_after() {
    let target = BlockPos::new(0, -60, 2);
    let behind = BlockPos::new(0, -60, 1);
    let mut region = ticking_with(&[(behind, lever(true))]);
    region.tick(&holding(items::REPEATER));
    region.tick(&click(relative(target, Face::Down)));
    let placed = block_at(&region, target).expect("loaded");
    assert_eq!(
        placed,
        repeater("north", 1, false),
        "facing the player's way"
    );
    idle(&mut region);
    assert_eq!(block_at(&region, target), Some(repeater("north", 1, true)));
}

/// A repeater with a powered repeater at its side is locked: it keeps its output when
/// its input goes, and lets go once the side does.
#[test]
fn a_repeater_locked_by_a_powered_repeater_at_its_side_keeps_its_output() {
    let main_lever = BlockPos::new(0, -60, 3);
    let main = BlockPos::new(1, -60, 3);
    let lamp = BlockPos::new(2, -60, 3);
    let side = BlockPos::new(1, -60, 4);
    let side_lever = BlockPos::new(1, -60, 5);
    let mut region = ticking_with(&[
        (main_lever, lever(false)),
        (main, repeater("west", 1, false)),
        (lamp, blocks::REDSTONE_LAMP),
        (side, repeater("south", 1, false)),
        (side_lever, lever(false)),
    ]);
    region.tick(&click(main_lever));
    idle(&mut region);
    idle(&mut region);
    assert!(powered(&region, main) && lit(&region, lamp));

    region.tick(&click(side_lever));
    idle(&mut region);
    idle(&mut region);
    assert!(powered(&region, side));
    assert_eq!(
        block_at(&region, main).and_then(|block| block.property("locked")),
        Some("true")
    );

    region.tick(&click(main_lever));
    for tick in 0..10 {
        idle(&mut region);
        assert!(powered(&region, main), "{tick}");
        assert!(lit(&region, lamp), "{tick}");
    }

    region.tick(&click(side_lever));
    idle(&mut region);
    idle(&mut region);
    assert_eq!(
        block_at(&region, main).and_then(|block| block.property("locked")),
        Some("false")
    );
    idle(&mut region);
    idle(&mut region);
    assert!(!powered(&region, main));
}

/// Two repeaters a lever powers at once, due in the same tick: the one told first (to
/// the lever's west, as neighbours are told west first) runs first, unless the other
/// faces a diode that does not face back into it, whose tick is scheduled at −3
/// against the first's −1 and runs before it (`DiodeBlock.checkTickOnNeighbor`).
#[test]
fn a_repeater_facing_a_diode_runs_before_one_told_before_it() {
    let at = BlockPos::new(3, -60, 3);
    let west = relative(at, Face::West);
    let east = relative(at, Face::East);
    let ahead = relative(east, Face::East);
    for prioritised in [false, true] {
        let mut blocks = vec![
            (at, lever(false)),
            (west, repeater("east", 1, false)),
            (east, repeater("west", 1, false)),
        ];
        if prioritised {
            blocks.push((ahead, repeater("north", 1, false)));
        }
        let mut region = ticking_with(&blocks);
        region.tick(&click(at));
        idle(&mut region);
        let output = idle(&mut region);
        let order: Vec<BlockPos> = changed(&output)
            .into_iter()
            .filter(|pos| *pos == west || *pos == east)
            .collect();
        let expected = if prioritised {
            [east, west]
        } else {
            [west, east]
        };
        assert_eq!(order, expected, "prioritised: {prioritised}");
    }
}

fn comparator(mode: &str, powered: bool) -> BlockState {
    state(&format!(
        "minecraft:comparator[facing=north,mode={mode},powered={powered}]"
    ))
}

/// A comparator facing north with a cake behind it: 14 out in compare mode against a
/// side of 13, which it is not below; in subtract mode 14 less 13. A click changes the
/// mode, its click heard by all but the player, higher for subtract.
#[test]
fn a_comparator_compares_and_subtracts_its_side_input() {
    let at = BlockPos::new(1, -60, 4);
    let cake = relative(at, Face::North);
    let out = relative(at, Face::South);
    let side_lever = BlockPos::new(5, -60, 4);
    let mut blocks = vec![
        (at, comparator("compare", false)),
        (out, blocks::REDSTONE_WIRE),
        (side_lever, lever(false)),
    ];
    for x in 2..=4 {
        blocks.push((BlockPos::new(x, -60, 4), blocks::REDSTONE_WIRE));
    }
    let mut region = ticking_with(&blocks);
    let mut output = TickOutput::default();
    region.set_block(cake, blocks::CAKE, &mut output);
    idle(&mut region);
    idle(&mut region);
    assert_eq!(block_at(&region, at), Some(comparator("compare", true)));
    assert_eq!(wire_power(&region, out), 14);

    region.tick(&click(side_lever));
    assert_eq!(wire_power(&region, BlockPos::new(2, -60, 4)), 13);
    idle(&mut region);
    idle(&mut region);
    assert_eq!(wire_power(&region, out), 14, "not below its side");

    let clicked = region.tick(&click(at));
    let comparator_click = clustine_data::sounds::BLOCK_COMPARATOR_CLICK;
    assert_eq!(
        sounds(&clicked),
        [(comparator_click, 0.3, 0.55, Some(player(1)))]
    );
    assert_eq!(block_at(&region, at), Some(comparator("subtract", true)));
    assert_eq!(wire_power(&region, out), 1, "at once, by the click");

    let clicked = region.tick(&click(at));
    assert_eq!(
        sounds(&clicked),
        [(comparator_click, 0.3, 0.5, Some(player(1)))]
    );
    assert_eq!(wire_power(&region, out), 14);
}

/// A comparator reads a cake behind a block behind it, and follows the cake's slices.
/// Its output is kept in its block entity, written as the game saves it whenever it
/// changes and said among the tick's changes, and read back from a saved chunk.
#[test]
fn a_comparator_reads_a_cake_through_a_block_and_keeps_its_output_durably() {
    let at = BlockPos::new(4, -60, 14);
    let block = relative(at, Face::North);
    let cake = relative(block, Face::North);
    let out = relative(at, Face::South);
    let mut region = ticking_with(&[
        (at, comparator("compare", false)),
        (block, blocks::STONE),
        (out, blocks::REDSTONE_WIRE),
    ]);
    let mut output = TickOutput::default();
    region.set_block(cake, blocks::CAKE, &mut output);
    idle(&mut region);
    let changed = idle(&mut region);
    assert_eq!(wire_power(&region, out), 14);
    assert_eq!(region.comparator_output(at), 14);
    let data = |output: u8| {
        let mut data = vec![10, 10, 0, 10];
        data.extend_from_slice(b"components");
        data.extend_from_slice(&[0, 3, 0, 12]);
        data.extend_from_slice(b"OutputSignal");
        data.extend_from_slice(&[0, 0, 0, output, 0]);
        data
    };
    let said: Vec<&RegionEvent> = changed
        .events
        .iter()
        .filter(|event| matches!(event, RegionEvent::BlockEntityChanged { .. }))
        .collect();
    assert_eq!(
        said,
        [&RegionEvent::BlockEntityChanged {
            position: at,
            data: data(14)
        }]
    );

    let mut output = TickOutput::default();
    region.set_block(cake, state("minecraft:cake[bites=3]"), &mut output);
    idle(&mut region);
    idle(&mut region);
    assert_eq!(wire_power(&region, out), 8);

    let saved = region.saved_chunk(at.chunk()).expect("loaded");
    let (x, z) = at.in_chunk();
    let entity = &saved.block_entities()[&InChunk::new(x, at.y, z)];
    assert_eq!(entity.data, data(8));

    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = around
        .iter()
        .map(|position| (*position, region.saved_chunk(*position).expect("loaded")))
        .collect();
    let ticketed: Vec<(ChunkPos, Ticket)> = around
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let mut restored = Region::restore_loaded(
        region.config.clone(),
        region.state(),
        Holdings {
            pinned: Vec::new(),
            held: around.clone(),
            lent: Vec::new(),
            lent_away: Vec::new(),
        },
        &ticketed,
        chunks,
        Vec::new(),
    );
    assert_eq!(restored.comparator_output(at), 8);
    // Wire placed in front of it takes its power from the output read back, once the
    // chunks tick.
    idle(&mut restored);
    let mut output = TickOutput::default();
    restored.set_block(out, blocks::AIR, &mut output);
    assert_eq!(block_at(&restored, out), Some(blocks::AIR));
    restored.set_block(out, blocks::REDSTONE_WIRE, &mut output);
    assert_eq!(wire_power(&restored, out), 8);
}

fn observer(facing: &str, powered: bool) -> BlockState {
    state(&format!(
        "minecraft:observer[facing={facing},powered={powered}]"
    ))
}

/// An observer placed does not pulse; when the block it faces changes it is powered two
/// ticks on, for two ticks, which the wire behind it shows.
#[test]
fn an_observer_pulses_two_ticks_when_the_block_it_faces_changes_and_not_when_placed() {
    let at = BlockPos::new(4, -60, 18);
    let watched = relative(at, Face::East);
    let behind = relative(at, Face::West);
    let mut region = ticking_with(&[(behind, blocks::REDSTONE_WIRE)]);
    let mut output = TickOutput::default();
    region.set_block(at, observer("east", false), &mut output);
    for _ in 0..6 {
        idle(&mut region);
        assert!(!powered(&region, at));
        assert_eq!(wire_power(&region, behind), 0);
    }
    assert!(!region.has_scheduled_tick(
        at,
        Ticked::Block(clustine_blocks::data::tick_id(observer("east", false)))
    ));

    region.set_block(watched, blocks::STONE, &mut output);
    idle(&mut region);
    assert!(!powered(&region, at));
    idle(&mut region);
    assert!(powered(&region, at));
    assert_eq!(wire_power(&region, behind), 15);
    idle(&mut region);
    assert!(powered(&region, at));
    idle(&mut region);
    assert!(!powered(&region, at));
    assert_eq!(wire_power(&region, behind), 0);
    for _ in 0..6 {
        idle(&mut region);
        assert!(!powered(&region, at));
    }
}

/// Two observers facing each other make a clock. By the jar's order each sees the
/// other's change only while it is unpowered and has no tick pending, so each is
/// powered for two ticks in every six.
#[test]
fn two_observers_facing_each_other_pulse_two_ticks_in_every_six() {
    let first = BlockPos::new(4, -60, 22);
    let second = relative(first, Face::East);
    let mut region = ticking_with(&[(first, observer("east", false))]);
    let mut output = TickOutput::default();
    region.set_block(second, observer("west", false), &mut output);
    let mut on = Vec::new();
    for tick in 1..=40u64 {
        idle(&mut region);
        if powered(&region, first) {
            on.push(tick);
        }
    }
    let starts: Vec<u64> = on
        .iter()
        .copied()
        .filter(|tick| !on.contains(&(tick - 1)))
        .collect();
    assert!(starts.len() >= 5, "{on:?}");
    for pair in starts.windows(2) {
        assert_eq!(pair[1] - pair[0], 6, "{on:?}");
    }
    for start in &starts[..starts.len() - 1] {
        assert!(
            on.contains(&(start + 1)) && !on.contains(&(start + 2)),
            "{on:?}"
        );
    }
}

/// A target block powered by nothing but arrows gives no signal before entities come,
/// and conducts: a lever on it powers it, which lights a lamp beside it.
#[test]
fn a_target_block_conducts_and_gives_no_signal_of_its_own() {
    let at = BlockPos::new(2, -60, 3);
    let lamp = relative(at, Face::North);
    let lever_at = relative(at, Face::East);
    let mut region = ticking_with(&[
        (at, blocks::TARGET),
        (lamp, blocks::REDSTONE_LAMP),
        (lever_at, wall_lever(false)),
    ]);
    region.tick(&click(lever_at));
    assert!(lit(&region, lamp));
    let mut view = crate::region::updates::RegionView(&mut region);
    assert_eq!(signal::own_signal(blocks::TARGET), 0);
    assert_eq!(
        signal::signal(&mut view, at, Face::South, signal::Query::ALL),
        15
    );
}
