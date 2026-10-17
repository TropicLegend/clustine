//! Sources and what power opens or lights (sections 1, 2, 7 and 11 of the record):
//! levers, buttons, wire, lamps, doors, trapdoors, gates, torches and their burnout,
//! copper bulbs, powered rails, note blocks, bells and pressure plates.

use clustine_data::sounds as s;

use super::*;
use crate::api::Face;

const LEVER: BlockPos = BlockPos::new(2, -60, 2);

/// A line of sixteen wires east of a lever: the game's wire loses one level a block, so
/// the first has 15 and the last none; turned off, the whole line is dark at the end of
/// the same tick, since wire works itself out within the update and schedules nothing.
#[test]
fn a_lever_lights_a_line_of_wire_from_fifteen_down_and_it_goes_dark_in_the_same_tick() {
    let line: Vec<BlockPos> = (3..=18).map(|x| at(x, -60, 2)).collect();
    let mut blocks = vec![(LEVER, lever(false))];
    blocks.extend(line.iter().map(|pos| (*pos, wire("east=side,west=side"))));
    let mut region = ticking_with(&blocks);
    step(&mut region, &click(LEVER));
    let powers: Vec<u8> = line.iter().map(|pos| power(&region, *pos)).collect();
    assert_eq!(powers, (0..=15).rev().collect::<Vec<u8>>());
    step(&mut region, &click(LEVER));
    assert!(line.iter().all(|pos| power(&region, *pos) == 0));
    // Nothing waits to happen afterwards.
    let later = watch(&mut region, 3, |region| power(region, line[0]));
    assert_eq!(later, [0, 0, 0]);
}

/// The lever's click: `LeverBlock.playSound`, at 0.3, pitch 0.6 turned on and 0.5 off,
/// played by the server to everyone, the player who clicked included (section 8).
#[test]
fn a_levers_click_is_heard_by_everyone_at_the_games_pitches() {
    let mut region = ticking_with(&[(LEVER, lever(false))]);
    let output = step(&mut region, &click(LEVER));
    assert_eq!(sounds(&output), [(s::BLOCK_LEVER_CLICK, 0.3, 0.6, None)]);
    let output = step(&mut region, &click(LEVER));
    assert_eq!(sounds(&output), [(s::BLOCK_LEVER_CLICK, 0.3, 0.5, None)]);
}

/// `RedstoneLampBlock`: lit in the update that powers it, put out by a tick four ticks
/// after its power goes.
#[test]
fn a_lamp_lights_at_once_and_goes_out_four_ticks_after_its_power() {
    let lamp = at(3, -60, 2);
    let mut region = ticking_with(&[(LEVER, lever(false)), (lamp, blocks::REDSTONE_LAMP)]);
    step(&mut region, &click(LEVER));
    assert!(on(&region, lamp, "lit"));
    step(&mut region, &click(LEVER));
    assert!(on(&region, lamp, "lit"), "the lamp waits for its tick");
    let lit = watch(&mut region, 5, |region| on(region, lamp, "lit"));
    assert_eq!(lit, [true, true, true, false, false]);
}

/// A lamp powered again before its tick stays lit: the tick finds it powered and does
/// nothing.
#[test]
fn a_lamp_powered_again_before_its_tick_stays_lit() {
    let lamp = at(3, -60, 2);
    let mut region = ticking_with(&[(LEVER, lever(false)), (lamp, blocks::REDSTONE_LAMP)]);
    step(&mut region, &click(LEVER));
    step(&mut region, &click(LEVER));
    idle(&mut region);
    step(&mut region, &click(LEVER));
    let lit = watch(&mut region, 6, |region| on(region, lamp, "lit"));
    assert_eq!(lit, [true; 6]);
}

/// `DoorBlock.neighborChanged`: a door powered at either half opens both halves and
/// marks both powered, in the update; unpowered, it closes.
#[test]
fn a_door_beside_a_lever_opens_both_halves_and_closes_with_it() {
    let (lower, upper) = (at(3, -60, 2), at(3, -59, 2));
    let door = |half: &str| {
        state(&format!(
            "minecraft:oak_door[facing=east,half={half},hinge=left,open=false,powered=false]"
        ))
    };
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (lower, door("lower")),
        (upper, door("upper")),
    ]);
    step(&mut region, &click(LEVER));
    for half in [lower, upper] {
        assert!(on(&region, half, "open"), "{half:?}");
        assert!(on(&region, half, "powered"), "{half:?}");
    }
    step(&mut region, &click(LEVER));
    for half in [lower, upper] {
        assert!(!on(&region, half, "open"), "{half:?}");
        assert!(!on(&region, half, "powered"), "{half:?}");
    }
}

/// Trapdoors and fence gates open and close with power in the update, as doors do.
#[test]
fn a_trapdoor_and_a_gate_open_and_close_with_power() {
    let trapdoor = at(3, -60, 2);
    let gate = at(2, -60, 3);
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (
            trapdoor,
            state("minecraft:oak_trapdoor[facing=north,half=bottom,open=false,powered=false]"),
        ),
        (
            gate,
            state("minecraft:oak_fence_gate[facing=east,open=false,powered=false]"),
        ),
    ]);
    step(&mut region, &click(LEVER));
    for pos in [trapdoor, gate] {
        assert!(on(&region, pos, "open"), "{pos:?}");
        assert!(on(&region, pos, "powered"), "{pos:?}");
    }
    step(&mut region, &click(LEVER));
    for pos in [trapdoor, gate] {
        assert!(!on(&region, pos, "open"), "{pos:?}");
        assert!(!on(&region, pos, "powered"), "{pos:?}");
    }
}

/// A button is let go by a tick of its block set's time pressed: 20 for stone, 30 for
/// wood (`BlockSetType.ticksToStayPressed`). Its click on is played for all but the
/// presser, whose client plays it; its click off for all.
#[test]
fn a_stone_button_stays_pressed_twenty_ticks_and_an_oak_button_thirty() {
    for (name, pressed, click_on, click_off) in [
        (
            "stone_button",
            20,
            s::BLOCK_STONE_BUTTON_CLICK_ON,
            s::BLOCK_STONE_BUTTON_CLICK_OFF,
        ),
        (
            "oak_button",
            30,
            s::BLOCK_WOODEN_BUTTON_CLICK_ON,
            s::BLOCK_WOODEN_BUTTON_CLICK_OFF,
        ),
    ] {
        let button = state(&format!(
            "minecraft:{name}[face=floor,facing=north,powered=false]"
        ));
        let lamp = at(3, -60, 2);
        let mut region = ticking_with(&[(LEVER, button), (lamp, blocks::REDSTONE_LAMP)]);
        let output = step(&mut region, &click(LEVER));
        assert_eq!(
            sounds(&output),
            [(click_on, 1.0, 1.0, Some(player(1)))],
            "{name}"
        );
        assert!(on(&region, LEVER, "powered") && on(&region, lamp, "lit"));
        let mut released = None;
        for tick in 1..=pressed + 10 {
            let output = idle(&mut region);
            if !on(&region, LEVER, "powered") && released.is_none() {
                released = Some(tick);
                assert_eq!(sounds(&output), [(click_off, 1.0, 1.0, None)], "{name}");
            }
            // The lamp goes out four ticks after the button.
            let lamp_lit = on(&region, lamp, "lit");
            assert_eq!(lamp_lit, tick < pressed + 4, "{name}, tick {tick}");
        }
        assert_eq!(released, Some(pressed), "{name}");
    }
}

/// A pressed button clicked again does nothing: `ButtonBlock.useWithoutItem` passes
/// over a powered button, and the release keeps its time.
#[test]
fn a_pressed_button_clicked_again_keeps_its_time() {
    let button = state("minecraft:stone_button[face=floor,facing=north,powered=false]");
    let mut region = ticking_with(&[(LEVER, button)]);
    step(&mut region, &click(LEVER));
    let pressed = watch(&mut region, 10, |region| on(region, LEVER, "powered"));
    assert_eq!(pressed, [true; 10]);
    let output = step(&mut region, &click(LEVER));
    assert_eq!(sounds(&output), []);
    let pressed = watch(&mut region, 10, |region| on(region, LEVER, "powered"));
    // Released in tick 20 after the first click: the eighth tick of these.
    assert_eq!(
        pressed,
        [true, true, true, true, true, true, true, true, false, false]
    );
}

/// The torch of these tests: on the east side of a stone block that a lever on top of
/// it powers.
const BASE: BlockPos = BlockPos::new(3, -60, 2);
const TOP: BlockPos = BlockPos::new(3, -59, 2);
const TORCH: BlockPos = BlockPos::new(4, -60, 2);

fn wall_torch(lit: bool) -> BlockState {
    state(&format!(
        "minecraft:redstone_wall_torch[facing=east,lit={lit}]"
    ))
}

fn torch_on_powered_block() -> Region {
    ticking_with(&[
        (BASE, blocks::STONE),
        (TOP, lever(false)),
        (TORCH, wall_torch(true)),
    ])
}

/// `RedstoneTorchBlock`: a neighbour's change schedules a tick of 2 where the torch's
/// light disagrees with its block's power, and the tick sets it.
#[test]
fn a_torch_goes_out_two_ticks_after_its_block_is_powered_and_lights_two_after() {
    let mut region = torch_on_powered_block();
    step(&mut region, &click(TOP));
    assert!(on(&region, TORCH, "lit"));
    let lit = watch(&mut region, 3, |region| on(region, TORCH, "lit"));
    assert_eq!(lit, [true, false, false]);
    step(&mut region, &click(TOP));
    assert!(!on(&region, TORCH, "lit"));
    let lit = watch(&mut region, 3, |region| on(region, TORCH, "lit"));
    assert_eq!(lit, [false, true, true]);
}

/// Burnout as the jar has it (`RedstoneTorchBlock.tick` and `isToggledTooFrequently`):
/// only a torch going out is counted, and the eighth time within 60 ticks puts it out
/// with level event 1502 and a tick 160 ticks on. A lever turned off then cannot light
/// it sooner: the neighbour's tick of 2 is dropped as the game drops a second tick of
/// one block at one position, and the tick of 160, all its toggles older than 60,
/// lights it.
#[test]
fn a_torch_put_out_eight_times_within_sixty_ticks_burns_out_and_lights_160_ticks_later() {
    let mut region = torch_on_powered_block();
    let mut lit = Vec::new();
    let mut burnt = Vec::new();
    let mut tick = 0u32;
    let mut advance = |region: &mut Region, inputs: &TickInputs| {
        let output = step(region, inputs);
        tick += 1;
        lit.push((tick, on(region, TORCH, "lit")));
        burnt.extend(level_events(&output).into_iter().map(|event| (tick, event)));
    };
    // Eight cycles, the lever on at 6i + 1 and off at 6i + 4: the torch goes out at
    // 6i + 3 and lights at 6i + 6, but for the eighth time out.
    for _ in 0..8 {
        advance(&mut region, &click(TOP));
        advance(&mut region, &TickInputs::default());
        advance(&mut region, &TickInputs::default());
        advance(&mut region, &click(TOP));
        advance(&mut region, &TickInputs::default());
        advance(&mut region, &TickInputs::default());
    }
    // Out until the tick of 160 after the eighth time.
    for _ in 48..45 + 160 + 2 {
        advance(&mut region, &TickInputs::default());
    }
    let changes: Vec<(u32, bool)> = lit
        .windows(2)
        .filter(|pair| pair[0].1 != pair[1].1)
        .map(|pair| pair[1])
        .collect();
    let mut expected = Vec::new();
    for i in 0..7 {
        expected.push((6 * i + 3, false));
        expected.push((6 * i + 6, true));
    }
    expected.extend([(45, false), (205, true)]);
    assert_eq!(changes, expected);
    assert_eq!(burnt, [(45, (1502, TORCH, 0))]);
}

/// A torch clock: a torch on the side of a block, its wire led up onto that block, so
/// that the torch powers its own block two ticks after it lights. It goes out every
/// four ticks, burns out the eighth time, and stays out: its last tick finds it toggled
/// too often and schedules nothing, and the tick of 160 that burning out asked for was
/// dropped for the tick of 2 its own going out had scheduled. A neighbour changed after
/// the toggles are older than 60 ticks lights it again, two ticks later.
#[test]
fn a_torch_clock_that_feeds_itself_burns_out_and_stays_out_until_a_neighbour_changes() {
    let block = at(3, -60, 3);
    let torch = at(4, -60, 3);
    let first = at(4, -60, 4);
    let second = at(3, -60, 4);
    let on_block = at(3, -59, 3);
    let mut region = ticking_with(&[
        (block, blocks::STONE),
        (first, wire("north=side,west=side")),
        (second, wire("east=side,north=up")),
        (on_block, wire("north=side,south=side")),
    ]);
    step(&mut region, &holding(items::REDSTONE_TORCH));
    let output = step(&mut region, &use_on(block, Face::East));
    assert_eq!(
        block_at(&region, torch),
        Some(wall_torch(true)),
        "placed lit"
    );
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::BlockChanged { position, .. } if *position == torch
    )));
    assert_eq!(power(&region, on_block), 13);
    let mut lit = Vec::new();
    let mut burnt = Vec::new();
    for tick in 1..=150 {
        let output = idle(&mut region);
        lit.push(on(&region, torch, "lit"));
        burnt.extend(level_events(&output).into_iter().map(|event| (tick, event)));
    }
    // Out at 2, 6, ..., 30; lit at 4, 8, ..., 28.
    for (index, lit) in lit.iter().enumerate() {
        let tick = index + 1;
        let expected = tick < 30 && tick % 4 <= 1;
        assert_eq!(*lit, expected, "tick {tick}");
    }
    assert_eq!(burnt, [(30, (1502, torch, 0))]);
    assert_eq!(power(&region, on_block), 0);
    // A block set beside the torch, long after the toggles are older than 60 ticks.
    step(&mut region, &holding(items::STONE));
    step(&mut region, &use_on(at(5, -61, 3), Face::Top));
    assert_eq!(block_at(&region, at(5, -60, 3)), Some(blocks::STONE));
    let lit = watch(&mut region, 6, |region| on(region, torch, "lit"));
    assert_eq!(lit, [false, true, true, false, false, true]);
}

/// `CopperBulbBlock.checkAndFlip`: in the update, a bulb whose power differs from its
/// `powered` takes the new power, and turns its light over only where it was not
/// powered before; it plays its turning on or off for all.
#[test]
fn a_copper_bulb_turns_over_on_each_rising_edge_and_not_when_power_goes() {
    let bulb = at(3, -60, 2);
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (
            bulb,
            state("minecraft:copper_bulb[lit=false,powered=false]"),
        ),
    ]);
    let lit_and_powered = |region: &Region| (on(region, bulb, "lit"), on(region, bulb, "powered"));
    let output = step(&mut region, &click(LEVER));
    assert_eq!(lit_and_powered(&region), (true, true));
    assert!(sound_ids(&output).contains(&s::BLOCK_COPPER_BULB_TURN_ON));
    let output = step(&mut region, &click(LEVER));
    assert_eq!(lit_and_powered(&region), (true, false));
    assert!(!sound_ids(&output).contains(&s::BLOCK_COPPER_BULB_TURN_OFF));
    let output = step(&mut region, &click(LEVER));
    assert_eq!(lit_and_powered(&region), (false, true));
    assert!(sound_ids(&output).contains(&s::BLOCK_COPPER_BULB_TURN_OFF));
    // Nothing is scheduled: a bulb flips in the update.
    let later = watch(&mut region, 3, lit_and_powered);
    assert_eq!(later, [(false, true); 3]);
}

/// A lit bulb's analog output is 15 (`CopperBulbBlock.getAnalogOutputSignal`), which a
/// comparator behind it reads.
#[test]
fn a_comparator_reads_fifteen_from_a_lit_copper_bulb() {
    let bulb = at(3, -60, 2);
    let comparator = at(4, -60, 2);
    let out = at(5, -60, 2);
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (
            bulb,
            state("minecraft:copper_bulb[lit=false,powered=false]"),
        ),
        (
            comparator,
            state("minecraft:comparator[facing=west,mode=compare,powered=false]"),
        ),
        (out, wire("east=side,west=side")),
    ]);
    step(&mut region, &click(LEVER));
    let powers = watch(&mut region, 3, |region| power(region, out));
    assert_eq!(powers, [0, 15, 15]);
}

/// `PoweredRailBlock.findPoweredRailSignal`: a rail is powered through up to eight
/// rails of its kind from the one beside the power, so a lever powers nine in a line.
#[test]
fn a_lever_powers_nine_powered_rails_in_a_line() {
    let rails: Vec<BlockPos> = (3..=13).map(|x| at(x, -60, 2)).collect();
    let mut blocks = vec![(LEVER, lever(false))];
    let rail = state("minecraft:powered_rail[powered=false,shape=east_west]");
    blocks.extend(rails.iter().map(|pos| (*pos, rail)));
    let mut region = ticking_with(&blocks);
    step(&mut region, &click(LEVER));
    let powered: Vec<bool> = rails
        .iter()
        .map(|pos| on(&region, *pos, "powered"))
        .collect();
    let mut expected = vec![true; 9];
    expected.extend([false, false]);
    assert_eq!(powered, expected);
    step(&mut region, &click(LEVER));
    assert!(rails.iter().all(|pos| !on(&region, *pos, "powered")));
}

fn note_block() -> BlockState {
    state("minecraft:note_block[instrument=basedrum,note=0,powered=false]")
}

/// `NoteBlock.neighborChanged`: a note block that becomes powered queues block event
/// (0, 0) and is marked powered; the event, run in the phase of block events, plays the
/// note (`triggerEvent`: volume 3, the pitch of note 0 is 0.5, of the records' source)
/// before the block event is sent. Power going plays nothing.
#[test]
fn a_powered_note_block_plays_its_note_in_the_phase_of_block_events() {
    let note = at(3, -60, 2);
    let mut region = ticking_with(&[(LEVER, lever(false)), (note, note_block())]);
    let output = step(&mut region, &click(LEVER));
    assert!(on(&region, note, "powered"));
    assert_eq!(
        block_events(&output),
        [(note, block_id(note_block()), 0, 0)]
    );
    let begin = output
        .events
        .iter()
        .position(|event| *event == RegionEvent::BlockEventsBegin)
        .expect("the phase is marked");
    let played = output
        .events
        .iter()
        .position(|event| matches!(event, RegionEvent::Sound { sound, .. } if *sound == s::BLOCK_NOTE_BLOCK_BASEDRUM))
        .expect("the note is played");
    let sent = output
        .events
        .iter()
        .position(|event| matches!(event, RegionEvent::BlockEvent { .. }))
        .expect("the event is sent");
    assert!(begin < played && played < sent, "{:?}", output.events);
    let note_sound = output.events.iter().find_map(|event| match event {
        RegionEvent::Sound {
            sound,
            source,
            volume,
            pitch,
            except,
            ..
        } if *sound == s::BLOCK_NOTE_BLOCK_BASEDRUM => Some((*source, *volume, *pitch, *except)),
        _ => None,
    });
    assert_eq!(
        note_sound,
        Some((crate::api::SOUND_SOURCE_RECORDS, 3.0, 0.5, None))
    );
    let output = step(&mut region, &click(LEVER));
    assert!(!on(&region, note, "powered"));
    assert_eq!(block_events(&output), []);
    assert!(!sound_ids(&output).contains(&s::BLOCK_NOTE_BLOCK_BASEDRUM));
}

/// A note block with a block on it plays nothing (`playNote`: the base drum does not
/// work under a block), though it is marked powered.
#[test]
fn a_note_block_under_a_block_is_powered_and_silent() {
    let note = at(3, -60, 2);
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (note, note_block()),
        (at(3, -59, 2), blocks::STONE),
    ]);
    let output = step(&mut region, &click(LEVER));
    assert!(on(&region, note, "powered"));
    assert_eq!(block_events(&output), []);
}

/// `NoteBlock.useWithoutItem`: a click raises the note by one, and plays it.
#[test]
fn clicking_a_note_block_raises_its_note_and_plays_it() {
    let note = at(3, -60, 2);
    let mut region = ticking_with(&[(note, note_block())]);
    let output = step(&mut region, &click(note));
    assert_eq!(property(&region, note, "note"), "1");
    assert_eq!(
        block_events(&output),
        [(note, block_id(note_block()), 0, 0)]
    );
    let pitch = sounds(&output)
        .into_iter()
        .find(|(sound, ..)| *sound == s::BLOCK_NOTE_BLOCK_BASEDRUM)
        .map(|(_, _, pitch, _)| pitch)
        .expect("played");
    assert!((pitch - 2f32.powf(-11.0 / 12.0)).abs() < 1e-6, "{pitch}");
}

/// `BellBlock.neighborChanged` and `BellBlockEntity.onHit`: a bell that becomes powered
/// rings, its sound at once (volume 2, pitch 1, for all) and block event (1, its
/// facing's 3D value) in the phase of block events.
#[test]
fn a_powered_bell_rings_with_a_block_event_naming_its_facing() {
    let bell = at(3, -60, 2);
    let floor_bell = state("minecraft:bell[attachment=floor,facing=north,powered=false]");
    let mut region = ticking_with(&[(LEVER, lever(false)), (bell, floor_bell)]);
    let output = step(&mut region, &click(LEVER));
    assert!(on(&region, bell, "powered"));
    assert!(sounds(&output).contains(&(s::BLOCK_BELL_USE, 2.0, 1.0, None)));
    assert_eq!(block_events(&output), [(bell, block_id(floor_bell), 1, 2)]);
    let output = step(&mut region, &click(LEVER));
    assert!(!on(&region, bell, "powered"));
    assert_eq!(block_events(&output), []);
}

/// A stone plate is pressed by a player standing on it at the end of the tick's
/// inputs, and checked again by a tick 20 ticks after it was pressed: a player who has
/// stepped off by then lets it go (`BasePressurePlateBlock`).
#[test]
fn a_stone_pressure_plate_is_pressed_by_a_player_and_let_go_by_its_tick_twenty_later() {
    let plate = at(2, -60, 2);
    let lamp = at(3, -60, 2);
    let mut region = ticking_with(&[
        (
            plate,
            state("minecraft:stone_pressure_plate[powered=false]"),
        ),
        (lamp, blocks::REDSTONE_LAMP),
    ]);
    step(&mut region, &moves(vec![walk(1, 2.5, 2.5)]));
    assert!(on(&region, plate, "powered"));
    assert!(on(&region, lamp, "lit"));
    let pressed = watch(&mut region, 4, |region| on(region, plate, "powered"));
    assert_eq!(pressed, [true; 4]);
    step(&mut region, &moves(vec![walk(1, 0.5, 0.5)]));
    let pressed = watch(&mut region, 16, |region| on(region, plate, "powered"));
    // Pressed in tick T, stepped off in T + 5: let go in T + 20.
    let mut expected = vec![true; 14];
    expected.extend([false, false]);
    assert_eq!(pressed, expected);
}

/// Wire's seven updates in the order of Java's `HashSet<BlockPos>` (section 2): wire
/// placed on a redstone block takes power 15 in its `onPlace` and updates the
/// neighbours of itself and of its six neighbours in the order of their buckets. A
/// placed wire alone is a cross, which points into the stone on each side; a lamp
/// beyond each stone is told only by that stone's position, and so lights in the
/// order of the positions. Worked out from the hash (`(z * 31 + y) * 31 + x`, spread
/// and masked to 16 buckets) at two places where the order differs.
#[test]
fn wire_updates_its_neighbours_in_the_order_of_javas_hash_set() {
    for (wire_at, order) in [
        (at(3, -60, 3), ["north", "west", "south", "east"]),
        (at(3, -60, 2), ["west", "south", "east", "north"]),
    ] {
        let offset = |side: &str, by: i32| match side {
            "north" => wire_at.offset(0, 0, -by),
            "south" => wire_at.offset(0, 0, by),
            "west" => wire_at.offset(-by, 0, 0),
            _ => wire_at.offset(by, 0, 0),
        };
        let mut blocks = vec![(wire_at.offset(0, -1, 0), blocks::REDSTONE_BLOCK)];
        for side in ["north", "south", "west", "east"] {
            blocks.push((offset(side, 1), blocks::STONE));
            blocks.push((offset(side, 2), blocks::REDSTONE_LAMP));
        }
        let mut region = ticking_with(&blocks);
        step(&mut region, &holding(items::REDSTONE));
        let output = step(&mut region, &use_on(wire_at.offset(0, -1, 0), Face::Top));
        assert_eq!(power(&region, wire_at), 15);
        let lamps: Vec<BlockPos> = order.iter().map(|side| offset(side, 2)).collect();
        let lit: Vec<BlockPos> = output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::BlockChanged { position, .. } if lamps.contains(position) => {
                    Some(*position)
                }
                _ => None,
            })
            .collect();
        assert_eq!(lit, lamps, "wire at {wire_at:?}");
    }
}

/// The burnout list is not durable (section 12): dropped at a restore. A torch clock
/// burnt out and restored lights again two ticks after a neighbour changes, though its
/// toggles are not 60 ticks old.
#[test]
fn a_burnt_out_torch_restored_lights_again_when_a_neighbour_changes() {
    let block = at(3, -60, 3);
    let torch = at(4, -60, 3);
    let mut region = ticking_with(&[
        (block, blocks::STONE),
        (at(4, -60, 4), wire("north=side,west=side")),
        (at(3, -60, 4), wire("east=side,north=up")),
        (at(3, -59, 3), wire("north=side,south=side")),
    ]);
    step(&mut region, &holding(items::REDSTONE_TORCH));
    step(&mut region, &use_on(block, Face::East));
    for _ in 0..35 {
        idle(&mut region);
    }
    assert!(!on(&region, torch, "lit"), "burnt out at 30");
    let mut restored = super::super::scheduled_ticks::restored(&region);
    step(&mut restored, &holding(items::STONE));
    step(&mut restored, &use_on(at(5, -61, 3), Face::Top));
    let lit = watch(&mut restored, 2, |region| on(region, torch, "lit"));
    assert_eq!(lit, [false, true]);
}

/// `RedStoneOreBlock.useItemOn`: a click lights the ore and, with a block in hand that
/// can be placed, passes, so the block is placed against it as well; with an item that
/// is not a block, the ore is lit and nothing is placed.
#[test]
fn clicking_redstone_ore_lights_it_and_still_places_a_block_in_hand() {
    let ore = at(3, -60, 2);
    let above = at(3, -59, 2);
    let mut region = ticking_with(&[(ore, state("minecraft:redstone_ore[lit=false]"))]);
    step(&mut region, &click(ore));
    assert!(on(&region, ore, "lit"));
    assert_eq!(block_at(&region, above), Some(blocks::STONE));
    let mut region = ticking_with(&[(ore, state("minecraft:redstone_ore[lit=false]"))]);
    step(&mut region, &holding(items::STICK));
    step(&mut region, &click(ore));
    assert!(on(&region, ore, "lit"));
    assert_eq!(block_at(&region, above), Some(blocks::AIR));
}

/// `RedStoneOreBlock.stepOn`: a player standing on redstone ore lights it.
#[test]
fn a_player_stepping_on_redstone_ore_lights_it() {
    let ore = at(2, -61, 2);
    let mut region = ticking_with(&[(ore, state("minecraft:redstone_ore[lit=false]"))]);
    step(&mut region, &moves(vec![walk(1, 2.5, 2.5)]));
    assert!(on(&region, ore, "lit"));
}

/// Section 8: the mark of the phase of block events is said once, in a tick that has
/// block events, before the first of them; the changes made from it on (a piston's
/// base set extended) come after it, the changes of the inputs (the lever) before. A
/// tick without block events has no mark.
#[test]
fn the_phase_of_block_events_is_marked_once_between_the_inputs_changes_and_its_own() {
    let piston = at(3, -60, 3);
    let lever_at = at(3, -60, 2);
    let mut region = ticking_with(&[
        (lever_at, lever(false)),
        (
            piston,
            state("minecraft:piston[extended=false,facing=east]"),
        ),
    ]);
    let output = step(&mut region, &click(lever_at));
    let marks: Vec<usize> = output
        .events
        .iter()
        .enumerate()
        .filter(|(_, event)| **event == RegionEvent::BlockEventsBegin)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(marks.len(), 1, "{:?}", output.events);
    let position_of = |pos: BlockPos| {
        output
            .events
            .iter()
            .position(|event| matches!(event, RegionEvent::BlockChanged { position, .. } if *position == pos))
            .expect("changed")
    };
    assert!(position_of(lever_at) < marks[0]);
    assert!(marks[0] < position_of(piston));
    let output = idle(&mut region);
    assert!(!output.events.contains(&RegionEvent::BlockEventsBegin));
}

/// `RepeaterBlock.isLocked` takes a diode's signal from the side, a comparator's too,
/// and not wire's.
#[test]
fn a_repeater_is_locked_by_a_comparator_at_its_side_and_not_by_wire() {
    let diode = at(3, -60, 2);
    let side = at(3, -60, 3);
    for (side_block, locks) in [
        (
            state("minecraft:comparator[facing=south,mode=compare,powered=false]"),
            true,
        ),
        (wire("north=side,south=side"), false),
    ] {
        let side_lever = at(3, -60, 4);
        let mut region = ticking_with(&[
            (
                diode,
                state("minecraft:repeater[delay=1,facing=west,locked=false,powered=false]"),
            ),
            (side, side_block),
            (side_lever, lever(false)),
        ]);
        step(&mut region, &click(side_lever));
        idle(&mut region);
        idle(&mut region);
        assert_eq!(on(&region, diode, "locked"), locks, "{side_block:?}");
    }
}
