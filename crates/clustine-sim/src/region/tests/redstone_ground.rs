//! Tests of the ground redstone stands on (step R2b of `docs/adr/0026-redstone.md`,
//! sections 2 and 3): the updater's single updates and its updates of six neighbours
//! but one, the update of comparators for an output signal, the queries of scheduled
//! ticks, and whether the region handles its tick.
//!
//! The blocks stand in the air in the middle of chunk (0, 0), the region's only loaded
//! chunk, where every neighbour can be read. What an update does is seen by what it
//! makes: water told of a change schedules its fluid tick, and a comparator with
//! nothing under it breaks.

use clustine_data::Face;
use clustine_world::{Seam, SeamKind, Ticked};

use super::*;
use crate::region::updates::{UPDATE_MOVE_BY_PISTON, UPDATE_NEIGHBORS, relative};

/// `clustine_blocks::fluid::WATER`.
const WATER: Ticked = Ticked::Fluid(2);

const MIDDLE: BlockPos = BlockPos::new(8, -55, 8);

fn beside(pos: BlockPos, face: Face) -> BlockPos {
    relative(pos, face)
}

const SIX: [Face; 6] = [
    Face::West,
    Face::East,
    Face::Down,
    Face::Up,
    Face::North,
    Face::South,
];

/// A region that ticks chunk (0, 0), with its eight neighbours loaded, and water on
/// every side of [`MIDDLE`] with no tick scheduled. A chunk that does not tick keeps
/// its ticks in itself, out of the queues the queries ask.
fn watered() -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    checked(&mut region, &tickets(around.clone(), vec![]));
    let chunks = around
        .into_iter()
        .map(|position| {
            let mut chunk = floor();
            if position == ChunkPos::new(0, 0) {
                for face in SIX {
                    let at = beside(MIDDLE, face);
                    let (x, z) = at.in_chunk();
                    chunk.set(x, at.y, z, blocks::WATER);
                }
            }
            (position, chunk)
        })
        .collect();
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks,
            ..TickInputs::default()
        },
    );
    region
}

fn ticking_water(region: &Region) -> Vec<Face> {
    SIX.into_iter()
        .filter(|face| region.has_scheduled_tick(beside(MIDDLE, *face), WATER))
        .collect()
}

#[test]
fn the_six_neighbours_are_told_of_a_change() {
    let mut region = watered();
    assert_eq!(ticking_water(&region), []);
    let mut output = TickOutput::default();
    region.update_neighbours_at(MIDDLE, blocks::STONE, &mut output);
    assert_eq!(ticking_water(&region), SIX);
}

#[test]
fn an_update_of_the_neighbours_but_one_passes_over_that_one_wherever_it_is_in_the_order() {
    for skip in SIX {
        let mut region = watered();
        let mut output = TickOutput::default();
        region.update_neighbours_at_except(MIDDLE, blocks::STONE, skip, &mut output);
        let told: Vec<Face> = SIX.into_iter().filter(|face| *face != skip).collect();
        assert_eq!(ticking_water(&region), told, "skipping {skip:?}");
    }
}

#[test]
fn a_single_update_tells_its_one_block() {
    let mut region = watered();
    let mut output = TickOutput::default();
    region.neighbour_changed_at(
        beside(MIDDLE, Face::Up),
        Face::Down,
        blocks::STONE,
        false,
        &mut output,
    );
    assert_eq!(ticking_water(&region), [Face::Up]);
}

/// A single update whose block cannot be read is kept, with the block that changed
/// and whether a piston moved it, in the chunk of the block it came from; at the
/// region's rim the block across is in a chunk not loaded.
#[test]
fn a_single_update_that_cannot_be_made_is_kept_with_its_source_and_its_flag() {
    let mut region = on_floor_with(&[]);
    let from = BlockPos::new(0, -55, 8);
    let across = BlockPos::new(-1, -55, 8);
    let mut output = TickOutput::default();
    region.neighbour_changed_at(across, Face::East, blocks::PISTON, true, &mut output);
    let seams = region.seams(ChunkPos::new(0, 0));
    assert_eq!(
        seams,
        [Seam {
            at: across,
            direction: Face::East as u8,
            kind: SeamKind::Neighbour,
            source: blocks::PISTON,
            flags: UPDATE_NEIGHBORS | UPDATE_MOVE_BY_PISTON,
            depth: 0,
        }],
        "kept beside {from:?}"
    );
}

/// `Level.updateNeighbourForOutputSignal`: a comparator beside, or beyond a conductor,
/// is told; one beyond a block that does not conduct is not.
#[test]
fn comparators_beside_or_beyond_a_conductor_are_told_of_an_output_signal() {
    let comparator = blocks::COMPARATOR;
    let north = beside(MIDDLE, Face::North);
    let east = beside(MIDDLE, Face::East);
    let beyond_east = beside(east, Face::East);
    let south = beside(MIDDLE, Face::South);
    let beyond_south = beside(south, Face::South);
    let mut region = on_ticking_floor_with(&[
        (north, comparator),
        (east, blocks::STONE),
        (beyond_east, comparator),
        (south, blocks::GLASS),
        (beyond_south, comparator),
    ]);
    let mut output = TickOutput::default();
    region.update_neighbour_for_output_signal(MIDDLE, blocks::CAKE, &mut output);
    let at = |pos: BlockPos| {
        let (x, z) = pos.in_chunk();
        region
            .chunk(pos.chunk())
            .and_then(|chunk| chunk.get(x, pos.y, z))
    };
    // Told, they find nothing under them and go.
    assert_eq!(at(north), Some(blocks::AIR));
    assert_eq!(at(beyond_east), Some(blocks::AIR));
    assert_eq!(at(beyond_south), Some(comparator));
}

/// `hasScheduledTick` is true of a tick pending and false once it has been drawn to
/// run; `willTickThisTick` is false outside the phase of ticks. A tick of stone does
/// nothing, so nothing schedules another.
#[test]
fn a_tick_is_scheduled_until_it_is_drawn_and_none_will_tick_between_ticks() {
    let mut region = watered();
    let stone = BlockPos::new(8, -61, 8);
    let of =
        Ticked::Block(u16::try_from(blocks::STONE.block_id().expect("a block")).expect("fits"));
    region.schedule_tick(stone, of, 2, 0);
    assert!(region.has_scheduled_tick(stone, of));
    assert!(!region.will_tick_this_tick(stone, of));
    idle(&mut region);
    assert!(region.has_scheduled_tick(stone, of), "due in two ticks");
    idle(&mut region);
    assert!(!region.has_scheduled_tick(stone, of), "drawn and run");
    assert!(!region.will_tick_this_tick(stone, of));
}

#[test]
fn a_region_does_not_handle_its_tick_between_ticks() {
    let mut region = on_floor_with(&[]);
    assert!(!region.handling_tick());
    idle(&mut region);
    assert!(!region.handling_tick());
}

// Block events (section 4 of the record).

/// A region that has loaded `chunks`, of the floor, with player 1 in it.
fn loaded_floor(chunks: &[ChunkPos]) -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    checked(&mut region, &tickets(chunks.to_vec(), vec![]));
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks.iter().map(|position| (*position, floor())).collect(),
            ..TickInputs::default()
        },
    );
    region
}

fn square(xs: std::ops::RangeInclusive<i32>) -> Vec<ChunkPos> {
    xs.flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A block of the floor, which the events are of.
fn stone_at(x: i32) -> BlockPos {
    BlockPos::new(x, -61, 8)
}

fn run(region: &Region) -> Vec<BlockPos> {
    region.events_run.iter().map(|event| event.at).collect()
}

#[test]
fn block_events_run_in_the_order_queued_once_each_where_their_block_is() {
    let mut region = loaded_floor(&square(-1..=1));
    region.block_event(stone_at(3), blocks::STONE, 0, 1);
    region.block_event(stone_at(1), blocks::STONE, 0, 1);
    // The same again is not queued twice; one with another number is.
    region.block_event(stone_at(3), blocks::STONE, 0, 1);
    region.block_event(stone_at(3), blocks::STONE, 0, 2);
    // The block there is not glass, so its event does nothing.
    region.block_event(stone_at(5), blocks::GLASS, 0, 1);
    idle(&mut region);
    assert_eq!(run(&region), [stone_at(3), stone_at(1), stone_at(3)]);
    let numbers: Vec<i32> = region.events_run.iter().map(|event| event.data).collect();
    assert_eq!(numbers, [1, 1, 2]);
    // Run, they are gone.
    region.events_run.clear();
    idle(&mut region);
    assert_eq!(run(&region), []);
}

/// An event whose chunk does not tick waits, kept in the chunk and committed once,
/// and runs in the tick the chunk starts ticking, when the chunk keeps it no more.
#[test]
fn a_block_event_of_a_chunk_that_does_not_tick_is_kept_in_it_until_it_ticks() {
    let mut region = loaded_floor(&square(-1..=2));
    // Chunk (2, 0) has no neighbours to the east loaded, and does not tick.
    let at = stone_at(40);
    region.block_event(at, blocks::STONE, 1, 3);
    let output = idle(&mut region);
    assert_eq!(run(&region), []);
    assert_eq!(output.events_changed, [ChunkPos::new(2, 0)]);
    let kept = region.kept_events(ChunkPos::new(2, 0));
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].kind, kept[0].data), (1, 3));
    // Waiting, it changes nothing to commit.
    let output = idle(&mut region);
    assert_eq!(output.events_changed, []);
    // With its eastern neighbours loaded the chunk ticks, and the event runs.
    let east = square(3..=3);
    checked(&mut region, &tickets(east.clone(), vec![]));
    let output = checked(
        &mut region,
        &TickInputs {
            chunks_loaded: east.iter().map(|position| (*position, floor())).collect(),
            ..TickInputs::default()
        },
    );
    assert_eq!(run(&region), [at]);
    assert_eq!(output.events_changed, [ChunkPos::new(2, 0)]);
    assert_eq!(region.kept_events(ChunkPos::new(2, 0)), []);
}

/// One queued after the phase, as between two ticks, runs in the next.
#[test]
fn a_block_event_queued_after_the_phase_runs_in_the_next_tick() {
    let mut region = loaded_floor(&square(-1..=1));
    idle(&mut region);
    region.block_event(stone_at(2), blocks::STONE, 0, 0);
    assert_eq!(run(&region), []);
    idle(&mut region);
    assert_eq!(run(&region), [stone_at(2)]);
}

/// A chunk unloaded takes its kept events out of the queue, and has them when it is
/// saved; loaded again, it puts them back at the queue's end.
#[test]
fn a_chunk_unloaded_takes_its_block_events_with_it_and_brings_them_back() {
    let mut region = loaded_floor(&square(-1..=2));
    let waiting = ChunkPos::new(2, 0);
    region.block_event(stone_at(40), blocks::STONE, 0, 0);
    idle(&mut region);
    let saved = region.saved_chunk(waiting).expect("loaded");
    assert_eq!(saved.events().len(), 1);
    checked(&mut region, &tickets(vec![], vec![waiting]));
    assert_eq!(region.block_events, []);
    // Back with its eastern neighbours, so that it ticks, and the event runs.
    let east = square(3..=3);
    let mut wanted = east.clone();
    wanted.push(waiting);
    checked(&mut region, &tickets(wanted.clone(), vec![]));
    let mut chunks: Vec<(ChunkPos, Chunk)> =
        east.iter().map(|position| (*position, floor())).collect();
    chunks.push((waiting, saved));
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks,
            ..TickInputs::default()
        },
    );
    assert_eq!(run(&region), [stone_at(40)]);
}

/// A region restored with its chunks, as after a merge or a split, queues their kept
/// events by chunk, ascending, and then in each chunk's order.
#[test]
fn a_region_restored_with_its_chunks_queues_their_events_by_chunk() {
    let chunks = square(-1..=1);
    let mut region = loaded_floor(&chunks);
    // Queued in the opposite order to the chunks', in two chunks that do not tick, so
    // that both are kept.
    // Chunks (3, 0) and (3, 1) without chunk 2 beside them: neither has all its
    // neighbours.
    let mut apart = square(-1..=1);
    apart.extend(square(3..=3));
    let mut far = loaded_floor(&apart);
    let later_chunk = BlockPos::new(56, -61, 24);
    far.block_event(later_chunk, blocks::STONE, 0, 0);
    far.block_event(stone_at(56), blocks::STONE, 0, 1);
    far.block_event(stone_at(57), blocks::STONE, 0, 2);
    idle(&mut far);
    assert_eq!(run(&far), [], "neither chunk ticks");
    let kept: Vec<(ChunkPos, Chunk)> = [ChunkPos::new(3, 0), ChunkPos::new(3, 1)]
        .into_iter()
        .map(|position| (position, far.saved_chunk(position).expect("loaded")))
        .collect();
    idle(&mut region);
    let all: Vec<(ChunkPos, Chunk)> = (-1..=4)
        .flat_map(|x| (-1..=2).map(move |z| ChunkPos::new(x, z)))
        .map(|position| {
            let chunk = kept
                .iter()
                .find(|(at, _)| *at == position)
                .map_or_else(floor, |(_, chunk)| chunk.clone());
            (position, chunk)
        })
        .collect();
    let ticketed: Vec<(ChunkPos, Ticket)> = all
        .iter()
        .map(|(position, _)| (*position, Ticket::Viewer))
        .collect();
    let mut restored = Region::restore_loaded(
        region.config.clone(),
        region.state(),
        Holdings {
            pinned: Vec::new(),
            held: ticketed.iter().map(|(position, _)| *position).collect(),
        },
        &ticketed,
        all,
        Vec::new(),
    );
    let order: Vec<i32> = restored
        .block_events
        .iter()
        .map(|event| event.data)
        .collect();
    assert_eq!(order, [1, 2, 0], "chunk (3, 0) before (3, 1)");
    idle(&mut restored);
    assert_eq!(run(&restored), [stone_at(56), stone_at(57), later_chunk]);
}

/// A block entity's new data is set in its chunk and said among the tick's changes,
/// after the block it belongs to; where there is no block entity nothing is said.
#[test]
fn a_block_entitys_data_is_said_in_its_place_among_the_changes() {
    let chest = BlockPos::new(4, -60, 4);
    let mut region = on_floor_with(&[]);
    let mut output = TickOutput::default();
    region.set_block(chest, blocks::CHEST, &mut output);
    assert!(region.set_block_entity_data(chest, b"items".to_vec(), &mut output));
    assert!(!region.set_block_entity_data(BlockPos::new(5, -60, 4), b"none".to_vec(), &mut output));
    let said: Vec<&RegionEvent> = output
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RegionEvent::BlockChanged { .. } | RegionEvent::BlockEntityChanged { .. }
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            &RegionEvent::BlockChanged {
                position: chest,
                state: blocks::CHEST
            },
            &RegionEvent::BlockEntityChanged {
                position: chest,
                data: b"items".to_vec()
            },
        ]
    );
    let saved = region.saved_chunk(chest.chunk()).expect("loaded");
    let (x, z) = chest.in_chunk();
    assert_eq!(
        saved.block_entities()[&InChunk::new(x, chest.y, z)].data,
        b"items".to_vec()
    );
}

// Clicks run the blocks' rules (section 7 of the record).

fn click_block(number: u128, at: BlockPos, face: Face) -> Input {
    numbered(
        number,
        PlayerInput::UseItemOn {
            position: at,
            face: match face {
                Face::Up => crate::api::Face::Top,
                Face::Down => crate::api::Face::Bottom,
                Face::North => crate::api::Face::North,
                Face::South => crate::api::Face::South,
                Face::West => crate::api::Face::West,
                Face::East => crate::api::Face::East,
            },
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    )
}

/// `DoorBlock.useWithoutItem` sets the door with `UPDATE_CLIENTS | UPDATE_IMMEDIATE`:
/// its neighbours are not told. A comparator floating beside it, which a neighbour's
/// update would take away, stays; both halves open.
#[test]
fn a_door_opened_by_hand_tells_no_neighbour_and_opens_both_halves() {
    let lower = BlockPos::new(2, -60, 3);
    let upper = BlockPos::new(2, -59, 3);
    let beside = BlockPos::new(3, -60, 3);
    let door = |half: &str, open: bool| {
        BlockState::parse(&format!(
            "minecraft:oak_door[half={half},facing=north,open={open}]"
        ))
        .expect("a door")
    };
    let mut region = on_ticking_floor_with(&[
        (lower, door("lower", false)),
        (upper, door("upper", false)),
        (beside, blocks::COMPARATOR),
    ]);
    let output = region.tick(&moves(vec![click_block(1, lower, Face::North)]));
    let at = |pos: BlockPos| {
        let (x, z) = pos.in_chunk();
        region
            .chunk(pos.chunk())
            .and_then(|chunk| chunk.get(x, pos.y, z))
    };
    assert_eq!(at(lower), Some(door("lower", true)), "{:?}", output.events);
    assert_eq!(at(upper), Some(door("upper", true)));
    assert_eq!(at(beside), Some(blocks::COMPARATOR));
}

/// `TrapDoorBlock.toggle`: the water in a waterlogged trapdoor looks again.
#[test]
fn a_waterlogged_trapdoor_used_has_its_water_tick() {
    let trapdoor = BlockPos::new(2, -60, 3);
    let closed = BlockState::parse("minecraft:oak_trapdoor[waterlogged=true]").expect("a trapdoor");
    let mut region = on_ticking_floor_with(&[(trapdoor, closed)]);
    assert!(!region.has_scheduled_tick(trapdoor, WATER));
    region.tick(&moves(vec![click_block(1, trapdoor, Face::Up)]));
    assert!(region.has_scheduled_tick(trapdoor, WATER));
}

/// A rail whose block goes is told so only where its chunk ticks: in a region whose
/// one chunk does not, the update waits as a seam entry, and the rail is still there;
/// once the chunk ticks, the entry runs and the rail goes
/// (`docs/adr/0026-redstone.md`, section 1). A rail goes only when told: its shape
/// rule takes nothing away, as wire's and a diode's take them away from below.
#[test]
fn a_rail_is_told_of_its_neighbour_only_where_its_chunk_ticks() {
    let block = BlockPos::new(2, -61, 3);
    let rail = BlockPos::new(2, -60, 3);
    let mut region = on_floor_with(&[(rail, blocks::RAIL)]);
    region.tick(&moves(vec![dig(1, block.x, block.y, block.z, 1)]));
    let at = |region: &Region, pos: BlockPos| {
        let (x, z) = pos.in_chunk();
        region
            .chunk(pos.chunk())
            .and_then(|chunk| chunk.get(x, pos.y, z))
    };
    assert_eq!(at(&region, block), Some(blocks::AIR));
    assert_eq!(at(&region, rail), Some(blocks::RAIL));
    assert!(
        region
            .seams(ChunkPos::new(0, 0))
            .iter()
            .any(|seam| seam.at == rail && seam.kind == SeamKind::Neighbour),
        "{:?}",
        region.seams(ChunkPos::new(0, 0))
    );
    // The eight chunks around come, and the chunk ticks.
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .filter(|position| *position != ChunkPos::new(0, 0))
        .collect();
    checked(&mut region, &tickets(around.clone(), vec![]));
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: around.iter().map(|position| (*position, floor())).collect(),
            ..TickInputs::default()
        },
    );
    assert_eq!(at(&region, rail), Some(blocks::AIR));
}
