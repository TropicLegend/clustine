//! Tests of points of interest in a region (`docs/groundwork/n10-plan.md`, section 3):
//! records that follow the blocks, queries in the fixed order, a ticket taken only in
//! the land, given back by an effect where the chunk is another region's, and how far
//! the village reaches.

use clustine_world::Effected;
use clustine_world::poi::PoiType;

use super::*;
use crate::region::pois::{NO_VILLAGE, Occupancy, section_of};

/// The floor at y = -61 above which blocks of the tests stand.
const Y: i32 = -60;

/// A region pinned to `area` holding the chunks from x = -2 to 3 and z = -2 to 2 that
/// are in it, with `blocks` set, and copies of `copied`.
fn region_of(area: ChunkArea, blocks: &[(BlockPos, BlockState)], copied: &[ChunkPos]) -> Region {
    let mut land: BTreeMap<ChunkPos, Chunk> = (-2..=3)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .map(|position| (position, floor()))
        .collect();
    for (at, state) in blocks {
        let chunk = land.get_mut(&at.chunk()).expect("within the land");
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    let chunks: Vec<(ChunkPos, Chunk)> = land
        .iter()
        .filter(|(position, _)| area.contains(**position))
        .map(|(position, chunk)| (*position, chunk.clone()))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = chunks
        .iter()
        .map(|(position, _)| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: chunks.iter().map(|(position, _)| *position).collect(),
        pinned: vec![area],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let copies = copied
        .iter()
        .map(|position| (*position, land[position].clone()))
        .collect();
    let state = RegionState::new(EntityIds::block(1).expect("a block of ids"));
    Region::restore_loaded(config(), state, holdings, &tickets, chunks, copies)
}

const EVERYWHERE: ChunkArea = ChunkArea {
    min_x: None,
    max_x: None,
};
/// Chunks x below 1, with a copy of x = 1.
const WEST: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};

fn bed_head() -> BlockState {
    blocks::RED_BED
        .with_property("part", "head")
        .expect("a bed has a part")
}

fn any(_: PoiType) -> bool {
    true
}

#[test]
fn a_query_goes_by_chunk_x_then_z_and_in_a_chunk_by_section_then_type_then_place() {
    let blocks = [
        (BlockPos::new(-3, Y, 2), blocks::LECTERN),
        (BlockPos::new(2, Y, 2), blocks::BELL),
        (BlockPos::new(1, Y, 2), blocks::LECTERN),
        (BlockPos::new(1, Y + 20, 1), blocks::COMPOSTER),
        (BlockPos::new(3, Y, -5), blocks::BARREL),
    ];
    let region = region_of(EVERYWHERE, &blocks, &[]);
    let found: Vec<BlockPos> = region
        .pois_in_square(BlockPos::new(0, Y, 0), 16, any, Occupancy::Any)
        .into_iter()
        .map(|record| record.pos)
        .collect();
    assert_eq!(
        found,
        [
            // Chunk (0, -1).
            BlockPos::new(3, Y, -5),
            // Chunk (-1, 0).
            BlockPos::new(-3, Y, 2),
            // Chunk (0, 0): the low section's librarian (8) before its bell (14), then
            // the section above.
            BlockPos::new(1, Y, 2),
            BlockPos::new(2, Y, 2),
            BlockPos::new(1, Y + 20, 1),
        ]
    );
    // In range is within the sphere; closest first sorts by distance, stably.
    let near: Vec<BlockPos> = region
        .pois_closest_first(BlockPos::new(1, Y, 1), 2, any, Occupancy::Any)
        .into_iter()
        .map(|record| record.pos)
        .collect();
    assert_eq!(near, [BlockPos::new(1, Y, 2), BlockPos::new(2, Y, 2)]);
}

#[test]
fn a_ticket_is_taken_in_the_land_once_and_given_back() {
    let lectern = BlockPos::new(5, Y, 5);
    let mut region = region_of(EVERYWHERE, &[(lectern, blocks::LECTERN)], &[]);
    let mut output = TickOutput::default();
    let taken = region.take_poi(
        BlockPos::new(0, Y, 0),
        48,
        |kind| kind.is_job_site(),
        |_| true,
        &mut output,
    );
    assert_eq!(taken.map(|record| record.pos), Some(lectern));
    assert_eq!(output.claims_changed, [ChunkPos::new(0, 0)]);
    assert!(
        region
            .take_poi(lectern, 48, any, |_| true, &mut output)
            .is_none(),
        "its one ticket is taken"
    );
    let occupied = region.pois_in_range(lectern, 1, any, Occupancy::IsOccupied);
    assert_eq!(occupied.len(), 1);

    let mut output = TickOutput::default();
    region.release_poi(lectern, PoiType::Librarian, &mut output);
    assert_eq!(output.claims_changed, [ChunkPos::new(0, 0)]);
    assert!(
        region
            .chunk(ChunkPos::new(0, 0))
            .unwrap()
            .claims()
            .is_empty()
    );
    // A release of a type that is not there, or of a ticket not taken, does nothing.
    let mut output = TickOutput::default();
    region.release_poi(lectern, PoiType::Librarian, &mut output);
    region.release_poi(lectern, PoiType::Mason, &mut output);
    assert!(output.claims_changed.is_empty());
}

#[test]
fn a_record_in_a_copy_is_read_but_has_no_space_and_a_release_there_is_sent() {
    let across = BlockPos::new(20, Y, 4);
    let mut region = region_of(WEST, &[(across, blocks::LECTERN)], &[ChunkPos::new(1, 0)]);
    let mut output = TickOutput::default();
    let found = region.pois_in_range(across, 4, any, Occupancy::Any);
    assert_eq!(found.len(), 1);
    assert!(!found[0].own);
    assert!(
        region
            .pois_in_range(across, 4, any, Occupancy::HasSpace)
            .is_empty(),
        "no ticket is taken across a line"
    );
    assert!(
        region
            .take_poi(across, 4, any, |_| true, &mut output)
            .is_none()
    );

    region.release_poi(across, PoiType::Librarian, &mut output);
    assert!(output.claims_changed.is_empty());
    assert!(matches!(
        region.made.last(),
        Some(effect) if effect.at == across
            && effect.what == Effected::Release { kind: PoiType::Librarian }
    ));
}

#[test]
fn a_block_that_changes_type_takes_its_claim_and_the_commit_says_so() {
    let lectern = BlockPos::new(5, Y, 5);
    let mut region = region_of(EVERYWHERE, &[(lectern, blocks::LECTERN)], &[]);
    let mut output = TickOutput::default();
    assert!(region.acquire_poi(lectern, &mut output));
    let mut output = TickOutput::default();
    region.set_block_updated(lectern, blocks::AIR, 3, 512, &mut output);
    assert_eq!(output.claims_changed, [ChunkPos::new(0, 0)]);
    assert!(
        region
            .chunk(ChunkPos::new(0, 0))
            .unwrap()
            .claims()
            .is_empty()
    );
}

#[test]
fn the_village_reaches_five_sections_from_an_occupied_record_in_three_dimensions() {
    let bed = BlockPos::new(8, Y, 8);
    let mut region = region_of(EVERYWHERE, &[(bed, bed_head())], &[]);
    let here = section_of(bed);
    assert_eq!(
        region.sections_to_village(here),
        NO_VILLAGE,
        "a free bed is no village"
    );
    let mut output = TickOutput::default();
    assert!(region.acquire_poi(bed, &mut output));
    assert_eq!(region.sections_to_village(here), 0);
    assert!(region.is_village(bed));
    for distance in 1..=5 {
        let up = [here[0], here[1] + distance, here[2]];
        assert_eq!(region.sections_to_village(up), distance);
        let diagonal = [here[0] + distance.min(3), here[1] - distance, here[2]];
        assert_eq!(region.sections_to_village(diagonal), distance);
    }
    assert_eq!(
        region.sections_to_village([here[0], here[1] + 6, here[2]]),
        NO_VILLAGE
    );
    assert!(region.is_close_to_village(BlockPos::new(8, Y + 32, 8), 2));
    assert!(!region.is_close_to_village(BlockPos::new(8, Y + 32, 8), 1));
    assert!(!region.is_close_to_village(BlockPos::new(8, Y + 16 * 7, 8), 6));
}

fn seen(at: BlockPos, kind: PoiType, occupied: bool) -> crate::api::PoiSeen {
    crate::api::PoiSeen { at, kind, occupied }
}

#[test]
fn a_region_reports_its_land_whole_first_then_only_what_changed() {
    let lectern = BlockPos::new(5, Y, 5);
    let mut region = region_of(EVERYWHERE, &[(lectern, blocks::LECTERN)], &[]);
    let mut output = TickOutput::default();
    region.report_village(true, &mut output);
    let report = output.village.expect("a first report");
    assert!(report.full);
    assert_eq!(report.pois.len(), 30, "every chunk, with records or none");
    assert!(report.pois.contains(&(
        ChunkPos::new(0, 0),
        vec![seen(lectern, PoiType::Librarian, false)]
    )));

    let mut output = TickOutput::default();
    region.report_village(false, &mut output);
    assert_eq!(output.village, None, "nothing changed");

    let mut output = TickOutput::default();
    assert!(region.acquire_poi(lectern, &mut output));
    region.report_village(false, &mut output);
    let report = output.village.expect("the claim is reported");
    assert!(!report.full);
    assert_eq!(
        report.pois,
        [(
            ChunkPos::new(0, 0),
            vec![seen(lectern, PoiType::Librarian, true)]
        )]
    );
}

#[test]
fn the_margin_is_read_occupied_but_never_with_space_and_counts_for_the_village() {
    let mut region = region_of(WEST, &[], &[]);
    let far = BlockPos::new(40, Y, 8);
    let bell = BlockPos::new(41, Y, 9);
    region.take_margin(&crate::api::PublishedVillage {
        whole: true,
        pois: vec![(
            ChunkPos::new(2, 0),
            vec![
                seen(far, PoiType::Home, true),
                seen(bell, PoiType::Meeting, false),
            ],
        )],
        dropped: Vec::new(),
        raids: Vec::new(),
    });
    let near = BlockPos::new(8, Y, 8);
    assert_eq!(region.pois_in_range(near, 48, any, Occupancy::Any).len(), 2);
    assert!(
        region
            .pois_in_range(near, 48, any, Occupancy::HasSpace)
            .is_empty()
    );
    assert_eq!(
        region.pois_in_range(near, 48, any, Occupancy::IsOccupied)[0].pos,
        far
    );
    assert_eq!(region.sections_to_village(section_of(near)), 2);

    // A later publication's changes are laid over it.
    region.take_margin(&crate::api::PublishedVillage {
        whole: false,
        pois: Vec::new(),
        dropped: vec![ChunkPos::new(2, 0)],
        raids: Vec::new(),
    });
    assert!(
        region
            .pois_in_range(near, 48, any, Occupancy::Any)
            .is_empty()
    );
    assert_eq!(region.sections_to_village(section_of(near)), NO_VILLAGE);
}

#[test]
fn publications_folded_before_a_tick_are_their_changes_in_order() {
    use crate::api::PublishedVillage;
    let chunk = |x| ChunkPos::new(x, 0);
    let one = |x| {
        (
            chunk(x),
            vec![seen(BlockPos::new(x * 16, Y, 0), PoiType::Home, true)],
        )
    };
    let mut folded = PublishedVillage {
        whole: false,
        pois: vec![one(1)],
        dropped: vec![chunk(2)],
        raids: Vec::new(),
    };
    folded.then(PublishedVillage {
        whole: false,
        pois: vec![one(2)],
        dropped: vec![chunk(1), chunk(3)],
        raids: Vec::new(),
    });
    assert_eq!(
        folded,
        PublishedVillage {
            whole: false,
            pois: vec![one(2)],
            dropped: vec![chunk(1), chunk(3)],
            raids: Vec::new(),
        }
    );
    // A whole one replaces what came before, and changes after it stay whole.
    folded.then(PublishedVillage {
        whole: true,
        pois: vec![one(4)],
        dropped: Vec::new(),
        raids: Vec::new(),
    });
    folded.then(PublishedVillage {
        whole: false,
        pois: vec![one(5)],
        dropped: vec![chunk(4)],
        raids: Vec::new(),
    });
    assert_eq!(
        folded,
        PublishedVillage {
            whole: true,
            pois: vec![one(5)],
            dropped: Vec::new(),
            raids: Vec::new(),
        }
    );
}
