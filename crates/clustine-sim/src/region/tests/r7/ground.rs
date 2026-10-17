//! What redstone stands on (sections 1 to 5 and 10 of the record): neighbours told of
//! the block that was there, the predicate "the chunk ticks" for updates, clicks,
//! placements, block events and tickers, block events kept in their chunks through
//! ticks, restores and the phase's end, and a piston that waits for its land.

use clustine_world::KeptEvent;

use super::*;
use crate::api::Face;

/// The eight chunks around the origin's.
fn around() -> Vec<ChunkPos> {
    (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .filter(|position| *position != ChunkPos::new(0, 0))
        .collect()
}

/// Loads the eight chunks around the origin's, floors all, in a region that has only
/// the origin's: from the second of the two ticks this takes, the origin's chunk ticks.
/// Returns the output of that tick.
fn load_around(region: &mut Region) -> TickOutput {
    step(region, &tickets(around(), vec![]));
    let output = step(
        region,
        &TickInputs {
            chunks_loaded: around().into_iter().map(|p| (p, floor())).collect(),
            ..TickInputs::default()
        },
    );
    assert!(region.ticks_chunk(ChunkPos::new(0, 0)));
    output
}

/// A region of player 1 with only the origin's chunk loaded, which so does not tick.
fn not_ticking(blocks: &[(BlockPos, BlockState)]) -> Region {
    let region = on_floor_with(blocks);
    assert!(!region.ticks_chunk(ChunkPos::new(0, 0)));
    region
}

const LEVER: BlockPos = BlockPos::new(3, -60, 2);
const JUNCTION: BlockPos = BlockPos::new(3, -60, 3);

fn rail(shape: &str) -> BlockState {
    state(&format!("minecraft:rail[shape={shape},waterlogged=false]"))
}

fn junction() -> Vec<(BlockPos, BlockState)> {
    vec![
        (at(2, -60, 3), rail("east_west")),
        (at(4, -60, 3), rail("east_west")),
        (at(3, -60, 4), rail("north_south")),
        (JUNCTION, rail("south_east")),
    ]
}

/// A change of block tells its neighbours of the block that was there
/// (`Level.setBlock` passes the old state's block): `RailBlock.updateState` turns a
/// junction of three only where that block is a signal source. A lever turned is a
/// lever before and after, and turns it, to south-west powered (the jar's
/// `RailState.place`: of south-east and south-west, the last that fits) and back to
/// south-east unpowered.
#[test]
fn a_junction_of_rails_turns_when_a_lever_beside_it_is_turned() {
    let mut blocks = junction();
    blocks.push((LEVER, lever(false)));
    let mut region = ticking_with(&blocks);
    step(&mut region, &click(LEVER));
    assert_eq!(property(&region, JUNCTION, "shape"), "south_west");
    step(&mut region, &click(LEVER));
    assert_eq!(property(&region, JUNCTION, "shape"), "south_east");
}

/// A redstone block set beside the junction replaces air, which is no signal source,
/// so the junction, though powered, does not turn.
#[test]
fn a_junction_of_rails_does_not_turn_for_a_redstone_block_set_beside_it() {
    let mut region = ticking_with(&junction());
    step(&mut region, &holding(items::REDSTONE_BLOCK));
    step(&mut region, &use_on(at(3, -61, 2), Face::Top));
    assert_eq!(block_at(&region, LEVER), Some(blocks::REDSTONE_BLOCK));
    assert_eq!(property(&region, JUNCTION, "shape"), "south_east");
}

/// A lever's use reads no signal, so it is clicked where the chunk does not tick; the
/// lamp it powers reads one, and its update waits as a seam entry until the chunk ticks
/// (section 1: nothing settles dark beside power for good).
#[test]
fn a_lever_turned_where_the_chunk_does_not_tick_lights_its_lamp_once_the_chunk_ticks() {
    let lamp = at(4, -60, 2);
    let mut region = not_ticking(&[(LEVER, lever(false)), (lamp, blocks::REDSTONE_LAMP)]);
    let output = step(&mut region, &click(LEVER));
    assert!(on(&region, LEVER, "powered"));
    assert!(!on(&region, lamp, "lit"));
    assert!(
        region
            .seams(ChunkPos::new(0, 0))
            .iter()
            .any(|seam| seam.at == lamp),
        "{:?}",
        region.seams(ChunkPos::new(0, 0))
    );
    assert!(output.seams_changed.contains(&ChunkPos::new(0, 0)));
    let later = watch(&mut region, 3, |region| on(region, lamp, "lit"));
    assert_eq!(later, [false; 3]);
    load_around(&mut region);
    assert!(on(&region, lamp, "lit"));
}

/// A click whose use reads a signal (the comparator's mode and output, wire's dot and
/// cross) is refused where the chunk does not tick: nothing changes, and the click is
/// acknowledged.
#[test]
fn clicks_that_read_a_signal_are_refused_where_the_chunk_does_not_tick() {
    let comparator = state("minecraft:comparator[facing=west,mode=compare,powered=false]");
    let dot = state("minecraft:redstone_wire[power=0]");
    let (comparator_at, wire_at) = (at(3, -60, 2), at(3, -60, 4));
    let mut region = not_ticking(&[(comparator_at, comparator), (wire_at, dot)]);
    for pos in [comparator_at, wire_at] {
        let output = step(&mut region, &click(pos));
        assert_eq!(block_changes(&output), [], "{pos:?}");
        assert!(output.player_events.contains(&acknowledged(1, 1)));
    }
    assert_eq!(block_at(&region, comparator_at), Some(comparator));
    assert_eq!(block_at(&region, wire_at), Some(dot));
    // Once the chunk ticks, the same clicks work.
    load_around(&mut region);
    step(&mut region, &click(comparator_at));
    assert_eq!(property(&region, comparator_at, "mode"), "subtract");
    step(&mut region, &click(wire_at));
    assert_ne!(block_at(&region, wire_at), Some(dot));
}

/// A placement whose state is worked out from a signal, or whose placing rules read
/// one, is refused where the target's chunk does not tick (section 1, and the record's
/// R3a and R4): a lamp, a trapdoor, a door, a gate, wire, a repeater, a comparator and
/// a piston. Stone, which reads nothing, is placed.
#[test]
fn placements_that_read_a_signal_are_refused_where_the_chunk_does_not_tick() {
    let floor_at = at(3, -61, 3);
    let target = at(3, -60, 3);
    for item in [
        items::REDSTONE_LAMP,
        items::OAK_TRAPDOOR,
        items::OAK_DOOR,
        items::OAK_FENCE_GATE,
        items::REDSTONE,
        items::REPEATER,
        items::COMPARATOR,
        items::PISTON,
    ] {
        let mut region = not_ticking(&[]);
        step(&mut region, &holding(item));
        let output = step(&mut region, &use_on(floor_at, Face::Top));
        assert_eq!(block_at(&region, target), Some(blocks::AIR), "item {item}");
        assert!(output.player_events.contains(&acknowledged(1, 1)));
        // In a chunk that ticks it is placed.
        load_around(&mut region);
        step(&mut region, &use_on(floor_at, Face::Top));
        assert_ne!(block_at(&region, target), Some(blocks::AIR), "item {item}");
    }
    let mut region = not_ticking(&[]);
    step(&mut region, &holding(items::STONE));
    step(&mut region, &use_on(floor_at, Face::Top));
    assert_eq!(block_at(&region, target), Some(blocks::STONE));
}

fn note_block(note: u8) -> BlockState {
    state(&format!(
        "minecraft:note_block[instrument=basedrum,note={note},powered=false]"
    ))
}

fn kept_events(region: &Region) -> Vec<KeptEvent> {
    region
        .chunk(ChunkPos::new(0, 0))
        .expect("loaded")
        .events()
        .to_vec()
}

fn kept(pos: BlockPos, block: BlockState) -> KeptEvent {
    let (x, z) = pos.in_chunk();
    KeptEvent {
        at: InChunk::new(x, pos.y, z),
        block: block_id(block),
        kind: 0,
        data: 0,
    }
}

/// A note block's use reads no signal and is clicked where the chunk does not tick;
/// its block event cannot run there and is put back, kept in the chunk in queue order,
/// the chunk named among those whose kept events changed only in ticks that change
/// them. An exact duplicate is dropped (the game's queue is a set). When the chunk
/// ticks, the events run in their order, each by its block as it is then.
#[test]
fn block_events_where_the_chunk_does_not_tick_wait_in_the_chunk_in_order() {
    let (first, second) = (at(3, -60, 2), at(5, -60, 2));
    let mut region = not_ticking(&[(first, note_block(0)), (second, note_block(0))]);
    let output = step(&mut region, &click(first));
    assert_eq!(property(&region, first, "note"), "1");
    assert_eq!(block_events(&output), []);
    assert_eq!(output.events_changed, [ChunkPos::new(0, 0)]);
    assert_eq!(kept_events(&region), [kept(first, note_block(0))]);
    let output = idle(&mut region);
    assert_eq!(output.events_changed, [], "not rewritten while it waits");
    let output = step(&mut region, &click(second));
    assert_eq!(output.events_changed, [ChunkPos::new(0, 0)]);
    let output = step(&mut region, &click(first));
    assert_eq!(property(&region, first, "note"), "2");
    assert_eq!(output.events_changed, [], "the same event again is dropped");
    assert_eq!(
        kept_events(&region),
        [kept(first, note_block(0)), kept(second, note_block(0))]
    );
    let output = load_around(&mut region);
    let id = block_id(note_block(0));
    assert_eq!(
        block_events(&output),
        [(first, id, 0, 0), (second, id, 0, 0)]
    );
    // The first plays the note it has now, 2.
    let pitches: Vec<f32> = sounds(&output)
        .into_iter()
        .map(|(.., pitch, _)| pitch)
        .collect();
    let expected = [2f32.powf(-10.0 / 12.0), 2f32.powf(-11.0 / 12.0)];
    assert_eq!(pitches.len(), 2, "{pitches:?}");
    for (pitch, expected) in pitches.iter().zip(expected) {
        assert!((pitch - expected).abs() < 1e-6, "{pitches:?}");
    }
    assert_eq!(kept_events(&region), []);
    assert_eq!(output.events_changed, [ChunkPos::new(0, 0)]);
}

/// The kept events are the queue's durable form: a region restored from its state and
/// its chunks' saved forms runs them when the chunk ticks.
#[test]
fn block_events_kept_in_a_chunk_outlive_a_restore() {
    let note = at(3, -60, 2);
    let mut region = not_ticking(&[(note, note_block(0))]);
    step(&mut region, &click(note));
    let mut restored = super::super::scheduled_ticks::restored(&region);
    assert_eq!(kept_events(&restored), [kept(note, note_block(0))]);
    let output = load_around(&mut restored);
    assert_eq!(
        block_events(&output),
        [(note, block_id(note_block(0)), 0, 0)]
    );
}

/// An event whose block is no longer there does nothing: a kept note block event whose
/// block was broken before its chunk ticks is dropped, not sent.
#[test]
fn a_kept_block_event_whose_block_is_gone_does_nothing() {
    let note = at(3, -60, 2);
    let mut region = not_ticking(&[(note, note_block(0))]);
    step(&mut region, &click(note));
    step(&mut region, &dig_at(note));
    assert_eq!(block_at(&region, note), Some(blocks::AIR));
    let output = load_around(&mut region);
    assert_eq!(block_events(&output), []);
    assert_eq!(kept_events(&region), []);
}

/// An event queued after the phase of block events, by a moving piston that finishes
/// in the phase of block entities and powers a note block, waits for the next tick,
/// kept in its chunk meanwhile (section 4).
#[test]
fn a_block_event_queued_after_the_phase_waits_for_the_next_tick() {
    let piston = at(3, -60, 3);
    let note = at(5, -60, 4);
    let mut region = ticking_with(&[
        (at(3, -60, 2), lever(false)),
        (
            piston,
            state("minecraft:piston[extended=false,facing=east]"),
        ),
        (at(4, -60, 3), blocks::REDSTONE_BLOCK),
        (note, note_block(0)),
    ]);
    step(&mut region, &click(at(3, -60, 2)));
    let output = idle(&mut region);
    assert_eq!(block_events(&output), []);
    let output = idle(&mut region);
    assert_eq!(
        block_at(&region, at(5, -60, 3)),
        Some(blocks::REDSTONE_BLOCK)
    );
    assert!(on(&region, note, "powered"));
    assert_eq!(block_events(&output), [], "queued after the phase");
    assert_eq!(kept_events(&region), [kept(note, note_block(0))]);
    assert!(output.events_changed.contains(&ChunkPos::new(0, 0)));
    let output = idle(&mut region);
    assert_eq!(
        block_events(&output),
        [(note, block_id(note_block(0)), 0, 0)]
    );
    assert_eq!(kept_events(&region), []);
}

/// A ticker in a chunk that does not tick waits (section 5): a moving piston whose
/// chunk stops ticking half way stays half way, and finishes once the chunk ticks
/// again, its two ticks left counted from then.
#[test]
fn a_moving_piston_waits_while_its_chunk_does_not_tick() {
    let piston = at(3, -60, 3);
    let ahead = at(5, -60, 3);
    let mut region = ticking_with(&[
        (at(3, -60, 2), lever(false)),
        (
            piston,
            state("minecraft:piston[extended=false,facing=east]"),
        ),
        (at(4, -60, 3), blocks::STONE),
    ]);
    step(&mut region, &click(at(3, -60, 2)));
    let corner = ChunkPos::new(-1, -1);
    let saved = region.saved_chunk(corner).expect("loaded");
    step(&mut region, &tickets(vec![], vec![corner]));
    assert!(!region.ticks_chunk(ChunkPos::new(0, 0)));
    let moving = watch(&mut region, 5, |region| {
        block_at(region, ahead).and_then(BlockState::block_id) == blocks::MOVING_PISTON.block_id()
    });
    assert_eq!(moving, [true; 5]);
    step(&mut region, &tickets(vec![corner], vec![]));
    step(
        &mut region,
        &TickInputs {
            chunks_loaded: vec![(corner, saved)],
            ..TickInputs::default()
        },
    );
    assert!(region.ticks_chunk(ChunkPos::new(0, 0)));
    assert_eq!(
        block_at(&region, ahead).and_then(BlockState::block_id),
        blocks::MOVING_PISTON.block_id(),
        "ticked once more: from half way to the end"
    );
    idle(&mut region);
    assert_eq!(block_at(&region, ahead), Some(blocks::STONE));
}

/// A piston whose move reaches a chunk that does not tick does not move (section 10):
/// its check is kept in its chunk as a waiting piston, not tried again by the seam
/// phase (so the chunk is not rewritten every tick), and tried again when the chunk its
/// move reaches starts ticking.
#[test]
fn a_piston_refused_where_its_move_reaches_a_chunk_that_does_not_tick_moves_when_it_ticks() {
    // The nine chunks around the origin's are loaded, so the origin's ticks and the
    // chunk to its east, whose eastern neighbours are not loaded, does not.
    let piston = at(14, -60, 3);
    let lever_at = at(14, -60, 2);
    let mut region = ticking_floor_with(&[
        (lever_at, lever(false)),
        (
            piston,
            state("minecraft:piston[extended=false,facing=east]"),
        ),
        (at(15, -60, 3), blocks::STONE),
    ]);
    assert!(region.ticks_chunk(ChunkPos::new(0, 0)));
    assert!(!region.ticks_chunk(ChunkPos::new(1, 0)));
    step(&mut region, &moves(vec![walk(1, 12.5, 2.5)]));
    let output = step(&mut region, &click(lever_at));
    assert!(on(&region, lever_at, "powered"));
    assert_eq!(block_events(&output), []);
    assert_eq!(
        block_at(&region, piston),
        Some(state("minecraft:piston[extended=false,facing=east]"))
    );
    let waiting: Vec<_> = region
        .seams(ChunkPos::new(0, 0))
        .into_iter()
        .filter(|seam| seam.at == piston && seam.flags & (1 << 30) != 0)
        .collect();
    assert_eq!(waiting.len(), 1, "{:?}", region.seams(ChunkPos::new(0, 0)));
    for _ in 0..3 {
        let output = idle(&mut region);
        assert_eq!(
            output.seams_changed,
            [],
            "a waiting piston rewrites nothing"
        );
        assert_eq!(block_events(&output), []);
    }
    let east: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(2, z)).collect();
    step(&mut region, &tickets(east.clone(), vec![]));
    let output = step(
        &mut region,
        &TickInputs {
            chunks_loaded: east.into_iter().map(|p| (p, floor())).collect(),
            ..TickInputs::default()
        },
    );
    assert!(region.ticks_chunk(ChunkPos::new(1, 0)));
    assert_eq!(
        block_events(&output),
        [(
            piston,
            block_id(state("minecraft:piston[extended=false,facing=east]")),
            0,
            5
        )]
    );
    idle(&mut region);
    idle(&mut region);
    assert_eq!(block_at(&region, at(16, -60, 3)), Some(blocks::STONE));
    assert!(
        region
            .seams(ChunkPos::new(0, 0))
            .iter()
            .all(|seam| seam.flags & (1 << 30) == 0)
    );
}

/// A region of player 1 with the chunks `loaded` loaded on the floor, `blocks` set in
/// them.
fn with_loaded(loaded: &[ChunkPos], blocks: &[(BlockPos, BlockState)]) -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    step(&mut region, &tickets(loaded.to_vec(), vec![]));
    let chunks = loaded
        .iter()
        .map(|position| {
            let mut chunk = floor();
            for (at, state) in blocks.iter().filter(|(at, _)| at.chunk() == *position) {
                let (x, z) = at.in_chunk();
                chunk.set(x, at.y, z, *state);
            }
            (*position, chunk)
        })
        .collect();
    step(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks,
            ..TickInputs::default()
        },
    );
    region
}

/// Section 4: the queue is the region's, in the order events were queued; a chunk
/// unloaded takes its events out of it, and loaded again puts its kept events at the
/// end. Two note blocks in two chunks that do not tick are clicked, the western first;
/// with the western chunk unloaded and loaded again in between, the eastern one's event
/// runs first once both chunks tick.
#[test]
fn a_chunk_loaded_again_puts_its_kept_events_at_the_end_of_the_queue() {
    let west = at(-3, -60, 2);
    let east = at(2, -60, 2);
    let west_chunk = ChunkPos::new(-1, 0);
    for reloaded in [false, true] {
        let mut region = with_loaded(
            &[west_chunk, ChunkPos::new(0, 0)],
            &[(west, note_block(0)), (east, note_block(0))],
        );
        assert!(!region.ticks_chunk(west_chunk) && !region.ticks_chunk(ChunkPos::new(0, 0)));
        step(&mut region, &click(west));
        step(&mut region, &click(east));
        if reloaded {
            let saved = region.saved_chunk(west_chunk).expect("loaded");
            assert_eq!(saved.events().len(), 1);
            step(&mut region, &tickets(vec![], vec![west_chunk]));
            assert!(region.chunk(west_chunk).is_none());
            step(&mut region, &tickets(vec![west_chunk], vec![]));
            step(
                &mut region,
                &TickInputs {
                    chunks_loaded: vec![(west_chunk, saved)],
                    ..TickInputs::default()
                },
            );
        }
        let rest: Vec<ChunkPos> = (-2..=1)
            .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
            .filter(|position| *position != west_chunk && *position != ChunkPos::new(0, 0))
            .collect();
        step(&mut region, &tickets(rest.clone(), vec![]));
        let output = step(
            &mut region,
            &TickInputs {
                chunks_loaded: rest.into_iter().map(|p| (p, floor())).collect(),
                ..TickInputs::default()
            },
        );
        let id = block_id(note_block(0));
        let expected = if reloaded {
            [(east, id, 0, 0), (west, id, 0, 0)]
        } else {
            [(west, id, 0, 0), (east, id, 0, 0)]
        };
        assert_eq!(block_events(&output), expected, "reloaded: {reloaded}");
    }
}

/// Section 2: an output-signal update reaches a comparator two blocks away through a
/// conductor (`Level.updateNeighbourForOutputSignal`). A copper bulb lit behind a stone
/// behind a comparator tells the comparator, which no neighbour update reaches.
#[test]
fn a_block_with_an_analog_output_tells_a_comparator_beyond_a_conductor() {
    let lever_at = at(1, -60, 1);
    let bulb = at(1, -60, 2);
    let comparator_at = at(3, -60, 2);
    let out = at(4, -60, 2);
    let mut region = ticking_with(&[
        (lever_at, lever(false)),
        (
            bulb,
            state("minecraft:copper_bulb[lit=false,powered=false]"),
        ),
        (at(2, -60, 2), blocks::STONE),
        (
            comparator_at,
            state("minecraft:comparator[facing=west,mode=compare,powered=false]"),
        ),
        (out, wire("east=side,west=side")),
    ]);
    step(&mut region, &click(lever_at));
    assert!(on(&region, bulb, "lit"));
    let powers = watch(&mut region, 3, |region| power(region, out));
    assert_eq!(powers, [0, 15, 15]);
}

/// Section 7: placement runs the blocks' `onPlace` and `setPlacedBy` and reads
/// signals. Beside a powered lever, a lamp is placed lit, a trapdoor open and powered,
/// a powered rail powered; a piston placed powered extends in the tick it is placed;
/// a repeater placed with its input powered ticks one tick later
/// (`DiodeBlock.setPlacedBy`).
#[test]
fn blocks_placed_beside_power_come_out_powered() {
    let lever_at = at(3, -60, 2);
    let target = at(3, -60, 3);
    let floor_at = at(3, -61, 3);
    for (item, check) in [
        (items::REDSTONE_LAMP, "lit"),
        (items::OAK_TRAPDOOR, "open"),
        (items::POWERED_RAIL, "powered"),
    ] {
        let mut region = ticking_with(&[(lever_at, lever(true))]);
        step(&mut region, &holding(item));
        step(&mut region, &use_on(floor_at, Face::Top));
        assert!(on(&region, target, check), "item {item}: {check}");
    }
    // Looking south, a piston is placed facing north, its face free.
    let mut region = ticking_with(&[(at(2, -60, 3), lever(true))]);
    step(&mut region, &holding(items::PISTON));
    let output = step(&mut region, &use_on(floor_at, Face::Top));
    assert_eq!(
        block_events(&output),
        [(
            target,
            block_id(state("minecraft:piston[extended=false,facing=north]")),
            0,
            2
        )]
    );
    assert_eq!(
        block_at(&region, target),
        Some(state("minecraft:piston[extended=true,facing=north]"))
    );
    // Looking south, a repeater is placed with its input to the north, the lever.
    let mut region = ticking_with(&[(lever_at, lever(true))]);
    step(&mut region, &holding(items::REPEATER));
    step(&mut region, &use_on(floor_at, Face::Top));
    assert_eq!(property(&region, target, "facing"), "north");
    assert!(!on(&region, target, "powered"));
    idle(&mut region);
    assert!(on(&region, target, "powered"));
}
