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
    let mut region = on_floor_with(&[
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
