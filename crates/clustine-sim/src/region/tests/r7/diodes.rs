//! Diodes and observers (sections 3, 5, 7 and 11 of the record): repeaters' delays,
//! locking and the priority of a diode that feeds another's side, comparators' modes,
//! side inputs and the analog outputs of the blocks the record names, their output kept
//! as a block entity, observers, and the circuits whose timing the game fixes: a
//! repeater clock and two observers facing each other.

use clustine_data::sounds as s;

use super::*;
use crate::api::Face;

const LEVER: BlockPos = BlockPos::new(2, -60, 2);
const DIODE: BlockPos = BlockPos::new(3, -60, 2);
const FRONT: BlockPos = BlockPos::new(4, -60, 2);

/// A repeater whose input is from the west and output to the east: a diode's `facing`
/// names the side its input comes from.
fn repeater(delay: u8) -> BlockState {
    state(&format!(
        "minecraft:repeater[delay={delay},facing=west,locked=false,powered=false]"
    ))
}

fn comparator(mode: &str) -> BlockState {
    state(&format!(
        "minecraft:comparator[facing=west,mode={mode},powered=false]"
    ))
}

/// `DiodeBlock`: a change of input schedules a tick of twice the delay, and the tick
/// sets the output; a lamp in front lights as the repeater does and goes out four ticks
/// after it. A click counts from the tick before its own, as the game handles a
/// player's packet between ticks (R8): the repeater turns a tick sooner after it.
#[test]
fn a_repeater_passes_power_on_after_two_game_ticks_for_each_step_of_its_delay() {
    for delay in 1..=4u8 {
        let mut region = ticking_with(&[
            (LEVER, lever(false)),
            (DIODE, repeater(delay)),
            (FRONT, blocks::REDSTONE_LAMP),
        ]);
        let ticks = usize::from(2 * delay);
        step(&mut region, &click(LEVER));
        let lit = watch(&mut region, ticks + 1, |region| {
            (on(region, DIODE, "powered"), on(region, FRONT, "lit"))
        });
        let mut expected = vec![(false, false); ticks - 2];
        expected.extend([(true, true); 3]);
        assert_eq!(lit, expected, "delay {delay}");
        step(&mut region, &click(LEVER));
        let lit = watch(&mut region, ticks + 4, |region| {
            (on(region, DIODE, "powered"), on(region, FRONT, "lit"))
        });
        let mut expected = vec![(true, true); ticks - 2];
        expected.extend([(false, true); 4]);
        expected.extend([(false, false); 2]);
        assert_eq!(lit, expected, "delay {delay}");
    }
}

/// A pulse of one tick into a repeater comes out as long as its delay: the tick that
/// turns it on finds no input and schedules its own turning off.
#[test]
fn a_repeater_stretches_a_pulse_of_one_tick_to_its_delay() {
    let mut region = ticking_with(&[(LEVER, lever(false)), (DIODE, repeater(3))]);
    step(&mut region, &click(LEVER));
    step(&mut region, &click(LEVER));
    let powered = watch(&mut region, 12, |region| on(region, DIODE, "powered"));
    // Watched from T + 2: on in T + 5, the click counting from T − 1, off in T + 11.
    let mut expected = vec![false; 3];
    expected.extend([true; 6]);
    expected.extend([false; 3]);
    assert_eq!(powered, expected);
}

/// A click cycles a repeater's delay from 1 to 4 and back, and the jar plays no sound
/// for it.
#[test]
fn clicking_a_repeater_cycles_its_delay_without_a_sound() {
    let mut region = ticking_with(&[(DIODE, repeater(1))]);
    let mut delays = Vec::new();
    for _ in 0..4 {
        let output = step(&mut region, &click(DIODE));
        assert_eq!(sounds(&output), []);
        delays.push(property(&region, DIODE, "delay"));
    }
    assert_eq!(delays, ["2", "3", "4", "1"]);
}

/// The side repeater of the locking tests: south of [`DIODE`], its output into the
/// diode's side, its input a lever further south.
const SIDE: BlockPos = BlockPos::new(3, -60, 3);
const SIDE_LEVER: BlockPos = BlockPos::new(3, -60, 4);

fn side_repeater() -> BlockState {
    state("minecraft:repeater[delay=1,facing=south,locked=false,powered=false]")
}

/// `RepeaterBlock`: a repeater whose side a powered diode feeds is locked, as its shape
/// says (`updateShape`) and as its ticks and neighbour changes work out again
/// (`isLocked`); a locked repeater keeps its output whatever its input does, and
/// unlocked it takes its input again.
#[test]
fn a_repeater_locked_from_the_side_keeps_its_output_until_it_is_unlocked() {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (DIODE, repeater(1)),
        (FRONT, blocks::REDSTONE_LAMP),
        (SIDE, side_repeater()),
        (SIDE_LEVER, lever(false)),
    ]);
    // The side repeater turns on in the tick after the click, which counts from the
    // tick before its own.
    step(&mut region, &click(SIDE_LEVER));
    let locked = watch(&mut region, 2, |region| on(region, DIODE, "locked"));
    assert_eq!(locked, [true, true]);
    step(&mut region, &click(LEVER));
    let state = watch(&mut region, 4, |region| {
        (on(region, DIODE, "powered"), on(region, FRONT, "lit"))
    });
    assert_eq!(state, [(false, false); 4], "locked, it does not turn on");
    step(&mut region, &click(SIDE_LEVER));
    let state = watch(&mut region, 4, |region| {
        (
            on(region, DIODE, "locked"),
            on(region, DIODE, "powered"),
            on(region, FRONT, "lit"),
        )
    });
    // The side repeater goes off in T + 1, which unlocks the diode and has it schedule
    // its tick, which turns it on in T + 3.
    assert_eq!(
        state,
        [
            (false, false, false),
            (false, false, false),
            (false, true, true),
            (false, true, true)
        ]
    );
}

/// `DiodeBlock.checkTickOnNeighbor` and `shouldPrioritize`: a diode whose front is a
/// diode facing another way ticks at the highest priority. One lever reaching both
/// repeaters, each of delay 1, has both tick in the same game tick; the one feeding the
/// other's side goes first, locks it, and the locked one never turns on.
#[test]
fn a_repeater_feeding_anothers_side_ticks_first_and_locks_it_before_it_turns_on() {
    let lever_at = at(2, -60, 3);
    let side_input = at(3, -60, 4);
    let mut region = ticking_with(&[
        (lever_at, lever(false)),
        (at(2, -60, 2), wire("east=side,south=side")),
        (at(2, -60, 4), wire("north=side,east=side")),
        (side_input, wire("west=side,north=side")),
        (DIODE, repeater(1)),
        (SIDE, side_repeater()),
        (FRONT, blocks::REDSTONE_LAMP),
    ]);
    step(&mut region, &click(lever_at));
    assert!(power(&region, at(2, -60, 2)) > 0 && power(&region, side_input) > 0);
    let state = watch(&mut region, 4, |region| {
        (
            on(region, SIDE, "powered"),
            on(region, DIODE, "locked"),
            on(region, DIODE, "powered"),
            on(region, FRONT, "lit"),
        )
    });
    // Both tick in the tick after the click, which counts from the tick before its own.
    assert_eq!(state, [(true, true, false, false); 4]);
}

/// The comparator of the next tests reads from the west and puts its output into a
/// wire to the east; its side input is wire from the north, fed by a lever beyond.
const SIDE_WIRE: BlockPos = BlockPos::new(3, -60, 1);
const SIDE_SOURCE: BlockPos = BlockPos::new(3, -60, -1);

fn comparing(mode: &str) -> Region {
    ticking_with(&[
        (LEVER, lever(false)),
        (DIODE, comparator(mode)),
        (FRONT, wire("east=side,west=side")),
        (SIDE_WIRE, wire("north=side,south=side")),
        (at(3, -60, 0), wire("north=side,south=side")),
        (SIDE_SOURCE, lever(false)),
    ])
}

/// `ComparatorBlock.calculateOutputSignal`: nothing from no input, nothing where the
/// side is stronger, the input less the side in subtraction, the input in comparison;
/// the output follows two ticks after a change of its inputs, a tick after a click's
/// own, which counts from the tick before.
#[test]
fn a_comparator_compares_or_subtracts_its_side_from_its_input() {
    for (mode, output) in [("compare", 15), ("subtract", 1)] {
        let mut region = comparing(mode);
        step(&mut region, &click(SIDE_SOURCE));
        assert_eq!(power(&region, SIDE_WIRE), 14);
        idle(&mut region);
        idle(&mut region);
        step(&mut region, &click(LEVER));
        let powers = watch(&mut region, 3, |region| power(region, FRONT));
        assert_eq!(powers, [output; 3], "{mode}");
        assert_eq!(on(&region, DIODE, "powered"), output > 0, "{mode}");
        // The side gone, both give the input.
        step(&mut region, &click(SIDE_SOURCE));
        let powers = watch(&mut region, 2, |region| power(region, FRONT));
        assert_eq!(powers, [15, 15], "{mode}");
    }
}

/// A side stronger than the input turns either mode off.
#[test]
fn a_comparator_gives_nothing_where_its_side_is_stronger_than_its_input() {
    for mode in ["compare", "subtract"] {
        let input_lever = at(0, -60, 2);
        let side_lever = at(3, -60, 0);
        let mut region = ticking_with(&[
            (input_lever, lever(false)),
            (at(1, -60, 2), wire("east=side,west=side")),
            (at(2, -60, 2), wire("east=side,west=side")),
            (DIODE, comparator(mode)),
            (FRONT, wire("east=side,west=side")),
            (SIDE_WIRE, wire("north=side,south=side")),
            (side_lever, lever(false)),
        ]);
        step(&mut region, &click(input_lever));
        idle(&mut region);
        idle(&mut region);
        assert_eq!(power(&region, FRONT), 14, "{mode}");
        step(&mut region, &click(side_lever));
        assert_eq!(power(&region, SIDE_WIRE), 15);
        let powers = watch(&mut region, 2, |region| power(region, FRONT));
        assert_eq!(powers, [0, 0], "{mode}");
    }
}

/// `ComparatorBlock.useWithoutItem`: a click turns the mode over, plays the click
/// (0.3, pitch 0.55 into subtraction and 0.5 back) for all but the player, and works
/// out the output at once, with no tick.
#[test]
fn clicking_a_comparator_turns_its_mode_and_its_output_at_once() {
    let mut region = comparing("compare");
    step(&mut region, &click(SIDE_SOURCE));
    step(&mut region, &click(LEVER));
    idle(&mut region);
    idle(&mut region);
    assert_eq!(power(&region, FRONT), 15);
    let output = step(&mut region, &click(DIODE));
    assert_eq!(property(&region, DIODE, "mode"), "subtract");
    assert_eq!(power(&region, FRONT), 1);
    assert_eq!(
        sounds(&output),
        [(s::BLOCK_COMPARATOR_CLICK, 0.3, 0.55, Some(player(1)))]
    );
    let output = step(&mut region, &click(DIODE));
    assert_eq!(property(&region, DIODE, "mode"), "compare");
    assert_eq!(power(&region, FRONT), 15);
    assert_eq!(
        sounds(&output),
        [(s::BLOCK_COMPARATOR_CLICK, 0.3, 0.5, Some(player(1)))]
    );
}

/// What a comparator puts out is its block entity's (section 5): each change of output
/// gives the block entity new data, said in the tick's events, with the game's
/// `OutputSignal`, and the chunk keeps it.
#[test]
fn a_comparators_output_is_kept_in_its_block_entity_and_said_when_it_changes() {
    let mut region = comparing("subtract");
    step(&mut region, &click(SIDE_SOURCE));
    step(&mut region, &click(LEVER));
    let output = idle(&mut region);
    let said: Vec<Option<i32>> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEntityChanged { position, data } if *position == DIODE => {
                Some(output_signal(data))
            }
            _ => None,
        })
        .collect();
    assert_eq!(said, [Some(1)]);
    let kept = block_entity_data(&region, DIODE).expect("a comparator has a block entity");
    assert_eq!(output_signal(&kept), Some(1));
    // The block entity comes in its place among the block changes: after the
    // comparator's own change of `powered`, which the jar sets after the output.
    let order: Vec<&RegionEvent> = output
        .events
        .iter()
        .filter(|event| {
            matches!(event, RegionEvent::BlockChanged { position, .. }
                | RegionEvent::BlockEntityChanged { position, .. } if *position == DIODE)
        })
        .collect();
    assert!(
        matches!(order.first(), Some(RegionEvent::BlockEntityChanged { .. })),
        "the output is set before the comparator is powered: {order:?}"
    );
    // A restored region reads it back: told of a neighbour, the comparator works out
    // the output it has, which differs from nothing it keeps, and schedules nothing.
    let mut restored = super::super::scheduled_ticks::restored(&region);
    idle(&mut restored);
    step(&mut restored, &use_on(at(3, -61, 3), Face::Top));
    assert_eq!(block_at(&restored, at(3, -60, 3)), Some(blocks::STONE));
    for _ in 0..3 {
        let output = idle(&mut restored);
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::BlockEntityChanged { .. })),
            "{:?}",
            output.events
        );
    }
    assert_eq!(power(&restored, FRONT), 1);
}

/// The analog output of the blocks without an inventory the record names (section
/// 11), read from the jar's `getAnalogOutputSignal`, as a comparator reads it behind
/// itself and, through a conductor, two behind.
#[test]
fn a_comparator_reads_the_analog_outputs_of_the_blocks_the_record_names() {
    let cases = [
        ("minecraft:cake[bites=0]", 14),
        ("minecraft:cake[bites=6]", 2),
        ("minecraft:composter[level=3]", 3),
        ("minecraft:composter[level=8]", 8),
        ("minecraft:water_cauldron[level=2]", 2),
        ("minecraft:powder_snow_cauldron[level=3]", 3),
        ("minecraft:lava_cauldron", 3),
        ("minecraft:end_portal_frame[eye=true,facing=north]", 15),
        ("minecraft:end_portal_frame[eye=false,facing=north]", 0),
        ("minecraft:respawn_anchor[charges=1]", 3),
        ("minecraft:respawn_anchor[charges=2]", 7),
        ("minecraft:respawn_anchor[charges=3]", 11),
        ("minecraft:respawn_anchor[charges=4]", 15),
        ("minecraft:beehive[facing=north,honey_level=4]", 4),
        ("minecraft:bee_nest[facing=north,honey_level=5]", 5),
    ];
    for (text, expected) in cases {
        for through in [false, true] {
            let behind = if through {
                at(1, -60, 2)
            } else {
                at(2, -60, 2)
            };
            let mut blocks = vec![
                (behind, state(text)),
                (DIODE, comparator("compare")),
                (FRONT, wire("east=side,west=side")),
            ];
            if through {
                blocks.push((at(2, -60, 2), blocks::STONE));
            }
            let mut region = ticking_with(&blocks);
            // A stone set beside the comparator tells it of its neighbours.
            step(&mut region, &use_on(at(3, -61, 1), Face::Top));
            assert_eq!(block_at(&region, at(3, -60, 1)), Some(blocks::STONE));
            // Its tick of 2 runs in the next tick, the placing counting from the tick
            // before its own.
            let powers = watch(&mut region, 3, |region| power(region, FRONT));
            assert_eq!(powers, [expected; 3], "{text}, through a block: {through}");
        }
    }
}

/// `ObserverBlock`: a change in front schedules a tick of 2, which powers the back for
/// two ticks; a placing counts from the tick before its own, so it powers in the next.
#[test]
fn an_observer_pulses_two_ticks_long_two_ticks_after_what_it_watches_changes() {
    let observer = state("minecraft:observer[facing=west,powered=false]");
    let mut region = ticking_with(&[(DIODE, observer), (FRONT, blocks::REDSTONE_LAMP)]);
    step(&mut region, &use_on(at(2, -61, 2), Face::Top));
    assert_eq!(block_at(&region, at(2, -60, 2)), Some(blocks::STONE));
    let state = watch(&mut region, 9, |region| {
        (on(region, DIODE, "powered"), on(region, FRONT, "lit"))
    });
    assert_eq!(
        state,
        [
            (true, true),
            (true, true),
            (false, true),
            (false, true),
            (false, true),
            (false, true),
            (false, false),
            (false, false),
            (false, false)
        ]
    );
}

/// Two observers facing each other, worked out from the jar's order of their ticks
/// (the record's R4 says every 6 ticks): one placed facing the other has that one
/// schedule a tick; each turning on is seen by the other, which is off and schedules
/// its own, ahead of the tick that turns the first off; each turning off is seen by the
/// other while it is on, and does nothing. So each is on for two ticks in six, the
/// second two ticks after the first, the first from the tick after the placing's (which
/// counts from the tick before its own). The official server does the same (R8).
#[test]
fn two_observers_facing_each_other_pulse_every_six_ticks() {
    let east = at(3, -60, 2);
    let west = at(4, -60, 2);
    let mut region =
        ticking_with(&[(east, state("minecraft:observer[facing=east,powered=false]"))]);
    step(&mut region, &holding(items::OBSERVER));
    // Looking west, the observer is placed watching west.
    step(&mut region, &turn(90.0));
    step(&mut region, &use_on(at(4, -61, 2), Face::Top));
    assert_eq!(
        block_at(&region, west),
        Some(state("minecraft:observer[facing=west,powered=false]"))
    );
    let state = watch(&mut region, 30, |region| {
        (on(region, east, "powered"), on(region, west, "powered"))
    });
    for (index, (east_on, west_on)) in state.into_iter().enumerate() {
        let tick = index + 1;
        assert_eq!(east_on, tick % 6 == 1 || tick % 6 == 2, "east, tick {tick}");
        assert_eq!(west_on, tick % 6 == 3 || tick % 6 == 4, "west, tick {tick}");
    }
}

/// A repeater clock: two repeaters of delay 1 in a ring of wire, started by a pulse of
/// one tick. The tick that turns one off (very high priority, scheduled by its own
/// tick) runs before the tick that turns the other on (high, scheduled by a neighbour's
/// change), so the pulse goes round every four ticks, two ticks on.
#[test]
fn a_repeater_clock_started_by_a_short_pulse_goes_round_every_four_ticks() {
    let first = at(3, -60, 3);
    let second = at(3, -60, 5);
    let lever_at = at(2, -60, 2);
    let mut region = ticking_with(&[
        (lever_at, lever(false)),
        (at(2, -60, 3), wire("north=side,east=side,south=side")),
        (
            first,
            state("minecraft:repeater[delay=1,facing=west,locked=false,powered=false]"),
        ),
        (at(4, -60, 3), wire("west=side,south=side")),
        (at(4, -60, 4), wire("north=side,south=side")),
        (at(4, -60, 5), wire("north=side,west=side")),
        (
            second,
            state("minecraft:repeater[delay=1,facing=east,locked=false,powered=false]"),
        ),
        (at(2, -60, 5), wire("east=side,north=side")),
        (at(2, -60, 4), wire("north=side,south=side")),
    ]);
    step(&mut region, &click(lever_at));
    step(&mut region, &click(lever_at));
    let state = watch(&mut region, 40, |region| {
        (on(region, first, "powered"), on(region, second, "powered"))
    });
    for (index, (first_on, second_on)) in state.into_iter().enumerate() {
        // Ticks after the pulse's first: the first repeater is on from T + 1, the
        // click counting from T − 1.
        let tick = index + 2;
        assert_eq!(
            first_on,
            tick >= 1 && (tick - 1) % 4 < 2,
            "first, T + {tick}"
        );
        assert_eq!(
            second_on,
            tick >= 3 && (tick - 3) % 4 < 2,
            "second, T + {tick}"
        );
    }
}
