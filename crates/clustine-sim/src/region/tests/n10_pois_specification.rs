//! Points of interest and the village in a region, tested from the specification by
//! someone who did not write the code: `docs/groundwork/n10-plan.md` (decisions 1, 2
//! and 15, sections 3.3 to 3.6), `docs/groundwork/n10-brief.md` (findings 1 to 6,
//! section 2.2), `docs/groundwork/n10-dumps.md` (part A: findings 1, 3, 5, 6 and the
//! tracker) and `docs/adr/0025-effects-between-regions.md`, "Changed in N10".
//!
//! The land is the floor at y = -61 in the chunks from x = -2 to 3 and z = -2 to 2.
//! Divided, the western region holds x up to 0 and copies x = 1; the eastern one holds
//! x from 1 and copies x = 0.

use std::cell::RefCell;

use clustine_world::poi::{Claim, PoiType};
use clustine_world::{Due, EffectId, Effected, FiledEffect, InChunk};

use super::*;
use crate::api::{PoiSeen, PublishedVillage};
use crate::region::Random;
use crate::region::pois::{NO_VILLAGE, Occupancy, PoiRecord, section_of};

/// The blocks just above the floor; section -4.
const Y: i32 = -60;

const EVERYWHERE: ChunkArea = ChunkArea {
    min_x: None,
    max_x: None,
};
const WESTERN: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EASTERN: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn chunk_at(x: i32, z: i32) -> ChunkPos {
    ChunkPos::new(x, z)
}

fn red_bed(part: &str) -> BlockState {
    blocks::RED_BED
        .with_property("part", part)
        .expect("a bed has a part")
}

/// A bed whose head is at `head` (facing north, so its foot is south of it).
fn bed(head: BlockPos) -> [(BlockPos, BlockState); 2] {
    [
        (head, red_bed("head")),
        (at(head.x, head.y, head.z + 1), red_bed("foot")),
    ]
}

/// The land, with `blocks` set and a ticket taken of each record at `claimed`
/// (`claimed` may name a record more than once, for a bell's tickets).
fn land(blocks: &[(BlockPos, BlockState)], claimed: &[BlockPos]) -> BTreeMap<ChunkPos, Chunk> {
    let mut land: BTreeMap<ChunkPos, Chunk> = (-2..=3)
        .flat_map(|x| (-2..=2).map(move |z| chunk_at(x, z)))
        .map(|position| (position, floor()))
        .collect();
    for (pos, state) in blocks {
        let chunk = land.get_mut(&pos.chunk()).expect("within the land");
        let (x, z) = pos.in_chunk();
        chunk.set(x, pos.y, z, *state);
    }
    for pos in claimed {
        let chunk = land.get_mut(&pos.chunk()).expect("within the land");
        let (x, z) = pos.in_chunk();
        assert!(chunk.acquire_ticket(InChunk::new(x, pos.y, z)), "{pos:?}");
    }
    land
}

/// A region pinned to `area` holding the chunks of `land` in it, for viewers, with
/// copies of `copied`.
fn region_of(area: ChunkArea, land: &BTreeMap<ChunkPos, Chunk>, copied: &[ChunkPos]) -> Region {
    region_with_ids(area, 1, land, copied)
}

fn region_with_ids(
    area: ChunkArea,
    ids: u32,
    land: &BTreeMap<ChunkPos, Chunk>,
    copied: &[ChunkPos],
) -> Region {
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
    let state = RegionState::new(EntityIds::block(ids).expect("a block of ids"));
    Region::restore_loaded(config(), state, holdings, &tickets, chunks, copies)
}

fn whole(blocks: &[(BlockPos, BlockState)], claimed: &[BlockPos]) -> Region {
    region_of(EVERYWHERE, &land(blocks, claimed), &[])
}

fn any(_: PoiType) -> bool {
    true
}

fn places(records: &[PoiRecord]) -> Vec<BlockPos> {
    records.iter().map(|record| record.pos).collect()
}

fn seen(pos: BlockPos, kind: PoiType, occupied: bool) -> PoiSeen {
    PoiSeen {
        at: pos,
        kind,
        occupied,
    }
}

fn claims_of(region: &Region, position: ChunkPos) -> Vec<Claim> {
    region
        .chunk(position)
        .or_else(|| region.copy(position))
        .expect("the region has the chunk")
        .claims()
        .to_vec()
}

fn in_chunk(pos: BlockPos) -> InChunk {
    let (x, z) = pos.in_chunk();
    InChunk::new(x, pos.y, z)
}

// The order of a query (decision 15; D10 part A, finding 6).

#[test]
fn a_query_walks_chunks_x_fastest_then_z_then_sections_up_then_type_id_then_y_z_x() {
    let blocks = [
        // Chunk (1, -1).
        (at(20, Y, -10), blocks::BARREL),
        // Chunk (-1, -1).
        (at(-4, Y, -4), blocks::LECTERN),
        // Chunk (-1, 0).
        (at(-3, Y, 5), blocks::SMOKER),
        // Chunk (0, 0), the section of the floor: a bell (14), three lecterns (8) and
        // a composter (4), set in no particular order.
        (at(2, Y, 2), blocks::BELL),
        (at(9, Y + 1, 0), blocks::LECTERN),
        (at(2, Y, 3), blocks::LECTERN),
        (at(5, Y, 0), blocks::LECTERN),
        (at(1, Y, 1), blocks::COMPOSTER),
        // Chunk (0, 0), the section above.
        (at(3, Y + 16, 3), blocks::BARREL),
    ];
    let region = whole(&blocks, &[]);
    let found = region.pois_in_square(at(8, Y, 8), 24, any, Occupancy::Any);
    assert_eq!(
        places(&found),
        [
            at(-4, Y, -4),
            at(20, Y, -10),
            at(-3, Y, 5),
            at(1, Y, 1),
            at(5, Y, 0),
            at(2, Y, 3),
            at(9, Y + 1, 0),
            at(2, Y, 2),
            at(3, Y + 16, 3),
        ]
    );
    // The same query asked again is the same: a function of the state.
    assert_eq!(
        found,
        region.pois_in_square(at(8, Y, 8), 24, any, Occupancy::Any)
    );
}

#[test]
fn in_square_is_a_box_at_any_height_and_in_range_a_ball_with_its_edge() {
    let blocks = [
        (at(13, Y, 13), blocks::LECTERN),
        (at(11, Y + 4, 8), blocks::LECTERN),
        (at(8, Y + 100, 8), blocks::LECTERN),
        (at(14, Y, 8), blocks::LECTERN),
        (at(3, Y, 8), blocks::LECTERN),
    ];
    let region = whole(&blocks, &[]);
    let centre = at(8, Y, 8);
    assert_eq!(
        places(&region.pois_in_square(centre, 5, any, Occupancy::Any)),
        [
            at(3, Y, 8),
            at(13, Y, 13),
            at(11, Y + 4, 8),
            at(8, Y + 100, 8)
        ]
    );
    // `distSqr <= r²`, in three dimensions: (3, 4, 0) away is 25, in; (5, 0, 5) is 50.
    assert_eq!(
        places(&region.pois_in_range(centre, 5, any, Occupancy::Any)),
        [at(3, Y, 8), at(11, Y + 4, 8)]
    );
}

#[test]
fn closest_first_is_a_stable_sort_and_the_closest_is_the_first_of_equals() {
    let blocks = [
        (at(6, Y, 8), blocks::LECTERN),
        (at(10, Y, 8), blocks::LECTERN),
        (at(8, Y, 10), blocks::LECTERN),
        (at(8, Y, 6), blocks::BELL),
        (at(8, Y, 9), blocks::BARREL),
        (at(8, Y, 5), blocks::COMPOSTER),
    ];
    let region = whole(&blocks, &[]);
    let centre = at(8, Y, 8);
    assert_eq!(
        places(&region.pois_closest_first(centre, 3, any, Occupancy::Any)),
        [
            at(8, Y, 9),
            // Three lecterns and a bell 2 away, in the query's order: type, then y,
            // z, x.
            at(6, Y, 8),
            at(10, Y, 8),
            at(8, Y, 10),
            at(8, Y, 6),
            at(8, Y, 5),
        ]
    );
    let closest = |wanted: fn(PoiType) -> bool| {
        region
            .poi_closest(centre, 3, wanted, Occupancy::Any)
            .map(|record| record.pos)
    };
    assert_eq!(closest(any), Some(at(8, Y, 9)));
    assert_eq!(
        closest(|kind| kind != PoiType::Fisherman),
        Some(at(6, Y, 8))
    );
    assert_eq!(
        closest(|kind| kind == PoiType::Meeting || kind == PoiType::Farmer),
        Some(at(8, Y, 6))
    );
    assert_eq!(closest(|kind| kind == PoiType::Home), None);
}

// Taking a ticket (decision 2; `PoiManager.take`).

#[test]
fn a_take_is_the_first_in_order_with_space_that_passes_its_test_and_only_in_the_land() {
    let land = land(
        &[
            // In the copy, the nearest of all.
            (at(17, Y, 8), blocks::LECTERN),
            // In the land: first in order but farther, then nearer.
            (at(6, Y, 3), blocks::LECTERN),
            (at(12, Y, 9), blocks::LECTERN),
        ],
        &[],
    );
    let mut region = region_of(WESTERN, &land, &[chunk_at(1, 0)]);
    let centre = at(14, Y, 8);
    let mut output = TickOutput::default();
    let first = region
        .take_poi(centre, 10, any, |_| true, &mut output)
        .expect("a lectern with space");
    assert_eq!(
        first.pos,
        at(6, Y, 3),
        "the first in order, not the nearest"
    );
    assert!(first.own);
    assert_eq!(output.claims_changed, [chunk_at(0, 0)]);
    assert_eq!(
        region
            .chunk(chunk_at(0, 0))
            .unwrap()
            .free_tickets(in_chunk(first.pos)),
        Some(0)
    );

    let second = region.take_poi(centre, 10, any, |_| true, &mut output);
    assert_eq!(second.map(|record| record.pos), Some(at(12, Y, 9)));
    assert_eq!(
        region.take_poi(centre, 10, any, |_| true, &mut output),
        None,
        "the copy's lectern is never taken here"
    );
    assert!(claims_of(&region, chunk_at(1, 0)).is_empty());

    // The test passes over the first: the next with space is taken.
    let mut fresh = region_of(WESTERN, &land, &[chunk_at(1, 0)]);
    let mut output = TickOutput::default();
    let passed = fresh.take_poi(
        centre,
        10,
        any,
        |record| record.pos != at(6, Y, 3),
        &mut output,
    );
    assert_eq!(passed.map(|record| record.pos), Some(at(12, Y, 9)));
    assert_eq!(
        fresh
            .chunk(chunk_at(0, 0))
            .unwrap()
            .free_tickets(in_chunk(at(6, Y, 3))),
        Some(1)
    );
}

#[test]
fn a_record_known_only_from_the_margin_is_never_taken() {
    let mut region = region_of(WESTERN, &land(&[], &[]), &[]);
    region.take_margin(&PublishedVillage {
        whole: true,
        pois: vec![(
            chunk_at(1, 0),
            vec![seen(at(20, Y, 8), PoiType::Librarian, false)],
        )],
        dropped: Vec::new(),
    });
    let mut output = TickOutput::default();
    assert_eq!(
        region
            .pois_in_range(at(14, Y, 8), 10, any, Occupancy::Any)
            .len(),
        1
    );
    assert!(
        region
            .pois_in_range(at(14, Y, 8), 10, any, Occupancy::HasSpace)
            .is_empty()
    );
    assert_eq!(
        region.take_poi(at(14, Y, 8), 10, any, |_| true, &mut output),
        None
    );
    assert!(output.claims_changed.is_empty());
}

#[test]
fn a_bell_gives_thirty_two_tickets_and_is_occupied_from_the_first() {
    let bell = at(4, Y, 4);
    let mut region = whole(&[(bell, blocks::BELL)], &[]);
    let mut output = TickOutput::default();
    assert!(
        region
            .pois_in_range(bell, 1, any, Occupancy::IsOccupied)
            .is_empty()
    );
    for taken in 1..=32 {
        let record = region.take_poi(bell, 4, any, |_| true, &mut output);
        assert_eq!(record.map(|record| record.pos), Some(bell), "{taken}");
        assert_eq!(
            places(&region.pois_in_range(bell, 1, any, Occupancy::IsOccupied)),
            [bell]
        );
    }
    assert_eq!(region.take_poi(bell, 4, any, |_| true, &mut output), None);
    assert!(
        region
            .pois_in_range(bell, 1, any, Occupancy::HasSpace)
            .is_empty()
    );
}

#[test]
fn a_random_record_is_drawn_as_util_to_shuffled_list_shuffles() {
    let blocks: Vec<(BlockPos, BlockState)> =
        (0..6).map(|n| (at(2 * n, Y, 3), blocks::LECTERN)).collect();
    let region = whole(&blocks, &[]);
    let centre = at(5, Y, 3);
    let in_order = places(&region.pois_in_range(centre, 48, any, Occupancy::Any));
    assert_eq!(in_order.len(), 6);

    let start = Random::of_block(7, centre, 11, 3);
    // The game's shuffle: for i from the count down to 2, swap i - 1 with nextInt(i).
    let mut expected = in_order.clone();
    let mut drawn = start.clone();
    for i in (2..=expected.len()).rev() {
        let j = drawn.next_int(i as u32) as usize;
        expected.swap(i - 1, j);
    }

    // Every record is tested in the shuffled order until one passes.
    let tested = RefCell::new(Vec::new());
    let mut random = start.clone();
    let none = region.random_poi(
        centre,
        48,
        any,
        Occupancy::Any,
        |record| {
            tested.borrow_mut().push(record.pos);
            false
        },
        &mut random,
    );
    assert_eq!(none, None);
    assert_eq!(tested.into_inner(), expected);
    assert_eq!(random.draws_since(&start), 5, "one draw fewer than records");
    assert_eq!(random, drawn);

    // The first passing is the answer, after the whole shuffle.
    let mut random = start.clone();
    let third = region.random_poi(
        centre,
        48,
        any,
        Occupancy::Any,
        |record| record.pos == expected[2] || record.pos == expected[4],
        &mut random,
    );
    assert_eq!(third.map(|record| record.pos), Some(expected[2]));
    assert_eq!(random, drawn);

    // One record, or none: no draw.
    let lone = whole(&[(at(1, Y, 1), blocks::LOOM)], &[]);
    let mut random = start.clone();
    assert!(
        lone.random_poi(at(1, Y, 1), 4, any, Occupancy::Any, |_| true, &mut random)
            .is_some()
    );
    assert!(
        lone.random_poi(
            at(1, Y, 1),
            4,
            |kind| kind == PoiType::Home,
            Occupancy::Any,
            |_| true,
            &mut random
        )
        .is_none()
    );
    assert_eq!(random.draws_since(&start), 0);
}

// Releases (decision 2; `Villager.releasePoi` guards `PoiManager.release`).

#[test]
fn a_release_in_the_land_is_guarded_by_the_type_there_and_a_ticket_taken() {
    let lectern = at(5, Y, 5);
    let bell = at(7, Y, 5);
    let mut region = whole(
        &[(lectern, blocks::LECTERN), (bell, blocks::BELL)],
        &[lectern, bell, bell],
    );
    let here = chunk_at(0, 0);

    let mut output = TickOutput::default();
    region.release_poi(lectern, PoiType::Mason, &mut output);
    region.release_poi(at(6, Y, 5), PoiType::Librarian, &mut output);
    assert!(
        output.claims_changed.is_empty(),
        "nothing of that type there"
    );
    assert_eq!(claims_of(&region, here).len(), 2);

    region.release_poi(lectern, PoiType::Librarian, &mut output);
    assert_eq!(output.claims_changed, [here]);
    let mut output = TickOutput::default();
    region.release_poi(lectern, PoiType::Librarian, &mut output);
    assert!(
        output.claims_changed.is_empty(),
        "every ticket free already"
    );

    region.release_poi(bell, PoiType::Meeting, &mut output);
    assert_eq!(output.claims_changed, [here]);
    assert_eq!(
        claims_of(&region, here),
        [Claim {
            at: in_chunk(bell),
            kind: PoiType::Meeting,
            free: 31
        }]
    );
    assert!(region.made.is_empty(), "nothing is sent for the land");
}

#[test]
fn a_release_in_a_copy_is_an_effect_and_changes_nothing_here() {
    let across = at(20, Y, 4);
    let land = land(&[(across, blocks::LECTERN)], &[across]);
    let mut region = region_of(WESTERN, &land, &[chunk_at(1, 0)]);
    let mut output = TickOutput::default();
    region.release_poi(across, PoiType::Librarian, &mut output);
    assert!(output.claims_changed.is_empty());
    assert_eq!(claims_of(&region, chunk_at(1, 0)).len(), 1);
    let releases: Vec<_> = region
        .made
        .iter()
        .filter(|effect| matches!(effect.what, Effected::Release { .. }))
        .collect();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].at, across);
    assert_eq!(
        releases[0].what,
        Effected::Release {
            kind: PoiType::Librarian
        }
    );
}

/// The plan's table (section 3.3): `release(pos)` is "in land: `releaseTicket`
/// (guarded); else `Effected::Release`", and section 3.6: "A release effect to land
/// nobody holds is filed by the store and applied when the chunk is next held". A
/// region pinned to the whole world that has not loaded a chunk does not hold it.
#[test]
#[ignore = "named: a release into land nobody has loaded is not made, and the store drops effects into chunks nobody uses (n10-plan.md, section 12)"]
fn a_release_in_land_the_region_has_not_loaded_is_sent_as_an_effect() {
    let mut region = whole(&[], &[]);
    let far = at(100, Y, 4);
    assert!(region.chunk(far.chunk()).is_none());
    let mut output = TickOutput::default();
    region.release_poi(far, PoiType::Home, &mut output);
    assert!(
        region.made.iter().any(|effect| effect.at == far
            && effect.what
                == Effected::Release {
                    kind: PoiType::Home
                }),
        "made: {:?}",
        region.made
    );
}

#[test]
fn a_block_changed_through_the_region_to_another_type_says_its_claims_changed_and_within_its_type_not()
 {
    let lectern = at(5, Y, 5);
    let composter = at(7, Y, 5);
    let mut region = whole(
        &[(lectern, blocks::LECTERN), (composter, blocks::COMPOSTER)],
        &[lectern, composter],
    );
    let here = chunk_at(0, 0);

    let mut output = TickOutput::default();
    let with_book = blocks::LECTERN
        .with_property("has_book", "true")
        .expect("a lectern may hold a book");
    region.set_block(lectern, with_book, &mut output);
    let fuller = blocks::COMPOSTER
        .with_property("level", "4")
        .expect("a composter has levels");
    region.set_block(composter, fuller, &mut output);
    assert!(output.claims_changed.is_empty());
    assert_eq!(claims_of(&region, here).len(), 2);

    let mut output = TickOutput::default();
    region.set_block(lectern, blocks::STONECUTTER, &mut output);
    assert_eq!(output.claims_changed, [here]);
    assert_eq!(
        claims_of(&region, here),
        [Claim {
            at: in_chunk(composter),
            kind: PoiType::Farmer,
            free: 0
        }]
    );
    // The stonecutter is a mason's site with its ticket free.
    let mason = region.pois_in_range(lectern, 0, any, Occupancy::HasSpace);
    assert_eq!(mason.len(), 1);
    assert_eq!(mason[0].kind, PoiType::Mason);
}

// The village (D10 part A, finding 3 corrected).

#[test]
fn the_village_is_the_chebyshev_distance_in_sections_up_to_five_and_seven_beyond() {
    let head = at(-5, Y, -5);
    let region = whole(&bed(head), &[head]);
    let here = section_of(head);
    assert_eq!(here, [-1, -4, -1]);
    for d in 0..=5 {
        assert_eq!(region.sections_to_village([-1 - d, -4, -1]), d, "west {d}");
        assert_eq!(
            region.sections_to_village([-1, -4 + d, -1 + d]),
            d,
            "up {d}"
        );
        assert_eq!(
            region.sections_to_village([-1 + d, -4 + d.min(2), -1 - d.min(3)]),
            d,
            "diagonal {d}"
        );
    }
    for far in [
        [-7, -4, -1],
        [5, -4, -1],
        [-1, 2, -1],
        [-1, -4, 5],
        [5, 2, 5],
    ] {
        assert_eq!(region.sections_to_village(far), NO_VILLAGE, "{far:?}");
    }
}

#[test]
fn the_nearest_of_several_villages_counts() {
    let head = at(-5, Y, -5);
    let bell = at(40, Y + 48, 8);
    let mut blocks = bed(head).to_vec();
    blocks.push((bell, blocks::BELL));
    let region = whole(&blocks, &[head, bell]);
    assert_eq!(section_of(bell), [2, -1, 0]);
    // Two from the bed, three from the bell.
    assert_eq!(region.sections_to_village([1, -4, -1]), 2);
    // Four from the bed, one from the bell.
    assert_eq!(region.sections_to_village([3, -2, 0]), 1);
}

#[test]
fn only_an_occupied_record_of_village_makes_a_village() {
    let bed_head = at(4, Y, 4);
    let mut blocks = bed(bed_head).to_vec();
    blocks.extend([
        (at(8, Y, 4), blocks::LECTERN),
        (at(10, Y, 4), blocks::BELL),
        (at(12, Y, 4), blocks::BEEHIVE),
        (at(13, Y, 4), blocks::BEE_NEST),
        (at(14, Y, 4), blocks::LIGHTNING_ROD),
        (at(15, Y, 4), blocks::LODESTONE),
    ]);
    let region = whole(&blocks, &[]);
    let here = section_of(bed_head);
    assert_eq!(region.sections_to_village(here), NO_VILLAGE, "all free");

    // The margin says a hive, a nest, a rod and a lodestone are occupied: no
    // record of `#village`, so no village, whatever it says.
    let mut region = whole(&blocks, &[]);
    region.take_margin(&PublishedVillage {
        whole: true,
        pois: vec![(
            chunk_at(4, 0),
            vec![
                seen(at(66, Y, 4), PoiType::Beehive, true),
                seen(at(67, Y, 4), PoiType::BeeNest, true),
                seen(at(68, Y, 4), PoiType::LightningRod, true),
                seen(at(69, Y, 4), PoiType::Lodestone, true),
            ],
        )],
        dropped: Vec::new(),
    });
    assert_eq!(region.sections_to_village(here), NO_VILLAGE);

    // One ticket of 32 taken is an occupied bell.
    let bell = at(10, Y, 4);
    let region = whole(&blocks, &[bell]);
    assert_eq!(region.sections_to_village(here), 0);
    let region = whole(&blocks, &[at(8, Y, 4)]);
    assert_eq!(region.sections_to_village(here), 0, "a claimed job site");
}

#[test]
fn the_village_is_read_from_the_land_the_copies_and_the_margin() {
    // A claimed bed in the copy of chunk (1, 0).
    let head = at(20, Y, 4);
    let copied = land(&bed(head), &[head]);
    let region = region_of(WESTERN, &copied, &[chunk_at(1, 0)]);
    assert_eq!(region.sections_to_village([-2, -4, 0]), 3);

    // An occupied bell known from the margin, four chunks beyond the copy.
    let mut region = region_of(WESTERN, &land(&[], &[]), &[chunk_at(1, 0)]);
    region.take_margin(&PublishedVillage {
        whole: true,
        pois: vec![(
            chunk_at(5, 1),
            vec![seen(at(85, Y + 16, 20), PoiType::Meeting, true)],
        )],
        dropped: Vec::new(),
    });
    assert_eq!(region.sections_to_village([0, -4, 0]), 5);
    assert_eq!(region.sections_to_village([-1, -4, 0]), NO_VILLAGE);
}

#[test]
fn close_to_a_village_needs_a_radius_of_six_at_most() {
    let head = at(4, Y, 4);
    let region = whole(&bed(head), &[head]);
    assert!(region.is_village(head));
    assert!(region.is_close_to_village(head, 0));
    assert!(region.is_close_to_village(head, 6));
    assert!(
        !region.is_close_to_village(head, 7),
        "r above 6 is never close"
    );
    assert!(!region.is_close_to_village(head, 100));
    let five_up = at(4, Y + 5 * 16, 4);
    assert_eq!(region.sections_to_village(section_of(five_up)), 5);
    assert!(region.is_close_to_village(five_up, 5));
    assert!(!region.is_close_to_village(five_up, 4));
    assert!(!region.is_village(five_up));
    let six_up = at(4, Y + 6 * 16, 4);
    assert!(!region.is_close_to_village(six_up, 6));
    // `isVillage` is within one section.
    let one_across = at(20, Y, 4);
    assert!(region.is_village(one_across));
    assert!(!region.is_village(at(36, Y, 4)));
}

// The margin (section 3.5) and copies (section 3.2).

#[test]
fn a_copy_and_the_land_are_read_never_the_margin_for_their_chunks() {
    let copied_lectern = at(20, Y, 4);
    let own_bed = at(4, Y, 4);
    let land = land(
        &[
            (copied_lectern, blocks::LECTERN),
            (own_bed, red_bed("head")),
            (at(4, Y, 5), red_bed("foot")),
        ],
        &[],
    );
    let mut region = region_of(WESTERN, &land, &[chunk_at(1, 0)]);
    // A margin that says otherwise of a copied chunk and of the land.
    region.take_margin(&PublishedVillage {
        whole: true,
        pois: vec![
            (chunk_at(0, 0), vec![seen(own_bed, PoiType::Home, true)]),
            (
                chunk_at(1, 0),
                vec![seen(copied_lectern, PoiType::Librarian, true)],
            ),
        ],
        dropped: Vec::new(),
    });
    let centre = at(12, Y, 4);
    assert!(
        region
            .pois_in_range(centre, 16, any, Occupancy::IsOccupied)
            .is_empty()
    );
    assert_eq!(
        places(&region.pois_in_range(centre, 16, any, Occupancy::Any)),
        [own_bed, copied_lectern],
        "each once"
    );
    assert_eq!(region.sections_to_village(section_of(centre)), NO_VILLAGE);
}

/// The plan's table (section 3.3): "`exists(pos, pred)`, `type_at(pos)` | | land,
/// copies, margin".
#[test]
fn the_type_at_a_place_is_read_from_the_margin_too() {
    let mut region = region_of(WESTERN, &land(&[], &[]), &[]);
    let far = at(40, Y, 8);
    region.take_margin(&PublishedVillage {
        whole: true,
        pois: vec![(chunk_at(2, 0), vec![seen(far, PoiType::Home, true)])],
        dropped: Vec::new(),
    });
    assert_eq!(region.poi_type_at(far), Some(PoiType::Home));
}

#[test]
fn the_margin_is_kept_across_ticks_and_each_publication_laid_over_it_in_order() {
    let mut region = region_of(WESTERN, &land(&[], &[]), &[]);
    let near = at(8, Y, 8);
    let bed_at = |x: i32| seen(at(x, Y, 8), PoiType::Home, true);
    let tick_with = |region: &mut Region, village: Option<PublishedVillage>| {
        region.tick(&TickInputs {
            village,
            ..TickInputs::default()
        })
    };
    tick_with(
        &mut region,
        Some(PublishedVillage {
            whole: true,
            pois: vec![
                (chunk_at(2, 0), vec![bed_at(40)]),
                (chunk_at(3, 0), vec![bed_at(56)]),
            ],
            dropped: Vec::new(),
        }),
    );
    let read = |region: &Region| places(&region.pois_in_range(near, 64, any, Occupancy::Any));
    assert_eq!(read(&region), [at(40, Y, 8), at(56, Y, 8)]);
    tick_with(&mut region, None);
    tick_with(&mut region, None);
    assert_eq!(read(&region), [at(40, Y, 8), at(56, Y, 8)], "kept");
    assert_eq!(region.sections_to_village(section_of(near)), 2);

    // Changes: chunk 2 dropped, chunk 4 added, chunk 3 changed.
    tick_with(
        &mut region,
        Some(PublishedVillage {
            whole: false,
            pois: vec![
                (
                    chunk_at(3, 0),
                    vec![seen(at(57, Y, 8), PoiType::Home, false)],
                ),
                (chunk_at(4, 0), vec![bed_at(70)]),
            ],
            dropped: vec![chunk_at(2, 0)],
        }),
    );
    assert_eq!(read(&region), [at(57, Y, 8), at(70, Y, 8)]);
    assert_eq!(region.sections_to_village(section_of(near)), 4);

    // A whole one replaces everything.
    tick_with(
        &mut region,
        Some(PublishedVillage {
            whole: true,
            pois: Vec::new(),
            dropped: Vec::new(),
        }),
    );
    assert_eq!(read(&region), []);
    assert_eq!(region.sections_to_village(section_of(near)), NO_VILLAGE);
}

#[test]
fn publications_folded_before_a_tick_are_one_publication_in_order() {
    let one = |x: i32, occupied: bool| {
        (
            chunk_at(x, 0),
            vec![seen(at(x * 16, Y, 0), PoiType::Home, occupied)],
        )
    };
    let fold = |publications: Vec<PublishedVillage>| {
        let mut publications = publications.into_iter();
        let mut folded = publications.next().expect("one at least");
        for later in publications {
            folded.then(later);
        }
        folded
    };
    let delta = |pois, dropped| PublishedVillage {
        whole: false,
        pois,
        dropped,
    };

    // Applied to a margin, the fold is what the publications one after the other are.
    let start = PublishedVillage {
        whole: true,
        pois: vec![one(1, true), one(2, true), one(3, true)],
        dropped: Vec::new(),
    };
    let sequence = vec![
        delta(vec![one(4, true), one(1, false)], vec![chunk_at(2, 0)]),
        delta(vec![one(2, false)], vec![chunk_at(4, 0), chunk_at(3, 0)]),
        delta(vec![one(3, false)], vec![chunk_at(1, 0)]),
    ];
    let mut one_by_one = region_of(WESTERN, &land(&[], &[]), &[]);
    let mut folded_in = region_of(WESTERN, &land(&[], &[]), &[]);
    one_by_one.take_margin(&start);
    folded_in.take_margin(&start);
    for publication in &sequence {
        one_by_one.take_margin(publication);
    }
    folded_in.take_margin(&fold(sequence.clone()));
    let read = |region: &Region| {
        region
            .pois_in_square(at(0, Y, 0), 80, any, Occupancy::IsOccupied)
            .len()
    };
    let all =
        |region: &Region| places(&region.pois_in_square(at(0, Y, 0), 80, any, Occupancy::Any));
    assert_eq!(all(&folded_in), all(&one_by_one));
    assert_eq!(all(&one_by_one), [at(32, Y, 0), at(48, Y, 0)]);
    assert_eq!(read(&folded_in), read(&one_by_one));
    assert_eq!(read(&one_by_one), 0);

    // A whole one in the middle replaces what came before it; what follows is laid
    // over it, and the fold stays whole.
    let folded = fold(vec![
        delta(vec![one(1, true)], Vec::new()),
        PublishedVillage {
            whole: true,
            pois: vec![one(2, true)],
            dropped: Vec::new(),
        },
        delta(vec![one(3, true)], vec![chunk_at(2, 0)]),
    ]);
    assert!(folded.whole);
    assert_eq!(folded.pois, [one(3, true)]);

    // A chunk dropped and then published again is published.
    let folded = fold(vec![
        delta(Vec::new(), vec![chunk_at(5, 0)]),
        delta(vec![one(5, false)], Vec::new()),
    ]);
    assert_eq!(folded.pois, [one(5, false)]);
    assert!(!folded.dropped.contains(&chunk_at(5, 0)));
}

#[test]
fn a_region_reports_its_land_whole_in_its_first_tick_then_only_chunks_that_changed() {
    let lectern = at(5, Y, 5);
    let land = land(&[(lectern, blocks::LECTERN)], &[]);
    let mut region = region_of(WESTERN, &land, &[chunk_at(1, 0)]);
    let first = region.tick(&TickInputs::default());
    let report = first.village.expect("the first tick reports");
    assert!(report.full);
    assert!(report.let_go.is_empty());
    let mut reported: Vec<ChunkPos> = report.pois.iter().map(|(p, _)| *p).collect();
    reported.sort_unstable();
    let mut held: Vec<ChunkPos> = land
        .keys()
        .filter(|position| WESTERN.contains(**position))
        .copied()
        .collect();
    held.sort_unstable();
    assert_eq!(
        reported, held,
        "every chunk of the land, and not the copies"
    );
    for (position, records) in &report.pois {
        if *position == chunk_at(0, 0) {
            assert_eq!(records, &[seen(lectern, PoiType::Librarian, false)]);
        } else {
            assert!(records.is_empty(), "{position:?}");
        }
    }

    assert_eq!(region.tick(&TickInputs::default()).village, None);

    // A record placed: its chunk alone, whole.
    let mut scratch = TickOutput::default();
    region.set_block(at(-3, Y, 2), blocks::LOOM, &mut scratch);
    let report = region
        .tick(&TickInputs::default())
        .village
        .expect("a record came");
    assert!(!report.full);
    assert_eq!(
        report.pois,
        [(
            chunk_at(-1, 0),
            vec![seen(at(-3, Y, 2), PoiType::Shepherd, false)]
        )]
    );

    // A ticket taken: its chunk alone, occupied.
    assert!(region.acquire_poi(lectern, &mut scratch));
    let report = region
        .tick(&TickInputs::default())
        .village
        .expect("the claim is reported");
    assert_eq!(
        report.pois,
        [(
            chunk_at(0, 0),
            vec![seen(lectern, PoiType::Librarian, true)]
        )]
    );
    // A region made again from the same chunks reports whole again.
    let mut again = region_of(WESTERN, &land, &[chunk_at(1, 0)]);
    assert!(again.tick(&TickInputs::default()).village.unwrap().full);
}

#[test]
fn a_bell_s_second_ticket_is_no_change_to_report() {
    let bell = at(5, Y, 5);
    let mut region = whole(&[(bell, blocks::BELL)], &[bell]);
    region.tick(&TickInputs::default());
    let mut scratch = TickOutput::default();
    assert!(region.acquire_poi(bell, &mut scratch));
    assert_eq!(
        region.tick(&TickInputs::default()).village,
        None,
        "still occupied, the same records"
    );
    region.release_poi(bell, PoiType::Meeting, &mut scratch);
    region.release_poi(bell, PoiType::Meeting, &mut scratch);
    let report = region.tick(&TickInputs::default()).village.unwrap();
    assert_eq!(
        report.pois,
        [(chunk_at(0, 0), vec![seen(bell, PoiType::Meeting, false)])]
    );
}

#[test]
fn a_copy_s_claims_are_laid_over_its_block_changes_of_the_same_tick() {
    let pos = at(20, Y, 4);
    let mut region = region_of(WESTERN, &land(&[], &[]), &[chunk_at(1, 0)]);
    region.tick(&TickInputs::default());
    region.tick(&TickInputs {
        copy_updates: vec![(chunk_at(1, 0), vec![(pos, blocks::LECTERN).into()])],
        copy_claims: vec![(
            chunk_at(1, 0),
            vec![Claim {
                at: in_chunk(pos),
                kind: PoiType::Librarian,
                free: 0,
            }],
        )],
        ..TickInputs::default()
    });
    assert_eq!(
        places(&region.pois_in_range(pos, 1, any, Occupancy::IsOccupied)),
        [pos]
    );
    assert_eq!(region.sections_to_village([0, -4, 0]), 1);

    // Claims alone, whole: the last ticket given back.
    region.tick(&TickInputs {
        copy_claims: vec![(chunk_at(1, 0), Vec::new())],
        ..TickInputs::default()
    });
    assert!(
        region
            .pois_in_range(pos, 1, any, Occupancy::IsOccupied)
            .is_empty()
    );
    assert_eq!(region.sections_to_village([0, -4, 0]), NO_VILLAGE);
}

// Two regions over a pinned line.

/// The east's effects, filed into the west's next tick (or the other way round).
fn filed(sequence: u64, effects: &[clustine_world::Effect], to: &Region) -> Vec<FiledEffect> {
    effects
        .iter()
        .filter(|effect| to.chunk(effect.at.chunk()).is_some())
        .enumerate()
        .map(|(index, effect)| FiledEffect {
            id: EffectId {
                sequence,
                index: u32::try_from(index).expect("few"),
            },
            effect: effect.clone(),
            due: Due::FromTaking,
        })
        .collect()
}

#[test]
fn a_take_in_one_region_is_read_occupied_in_the_other_s_copy_and_a_release_from_there_is_applied() {
    let lectern = at(20, Y, 4);
    let head = at(22, Y, 4);
    let mut blocks = vec![(lectern, blocks::LECTERN)];
    blocks.extend(bed(head));
    let land = land(&blocks, &[]);
    let mut west = region_with_ids(WESTERN, 1, &land, &[chunk_at(1, 0)]);
    let mut east = region_with_ids(EASTERN, 2, &land, &[chunk_at(0, 0)]);
    west.tick(&TickInputs::default());
    east.tick(&TickInputs::default());
    let across = chunk_at(1, 0);

    // The east takes both tickets in its land; the west cannot take either.
    let mut output = TickOutput::default();
    assert_eq!(west.take_poi(lectern, 8, any, |_| true, &mut output), None);
    let mut taken = TickOutput::default();
    assert!(
        east.take_poi(lectern, 1, |kind| kind.is_job_site(), |_| true, &mut taken)
            .is_some()
    );
    assert!(
        east.take_poi(head, 1, |kind| kind == PoiType::Home, |_| true, &mut taken)
            .is_some()
    );
    assert_eq!(taken.claims_changed, [across, across]);

    // What the store would send the west: the copy's claims, whole.
    let claims = claims_of(&east, across);
    assert_eq!(claims.len(), 2);
    west.tick(&TickInputs {
        copy_claims: vec![(across, claims)],
        ..TickInputs::default()
    });
    assert_eq!(
        places(&west.pois_in_range(lectern, 4, any, Occupancy::IsOccupied)),
        [lectern, head]
    );
    assert!(west.is_village(at(8, Y, 4)));

    // The west gives the lectern's ticket back: an effect the east applies.
    let mut output = TickOutput::default();
    west.release_poi(lectern, PoiType::Librarian, &mut output);
    let sent = west.tick(&TickInputs::default());
    let releases: Vec<_> = sent
        .effects
        .iter()
        .filter(|effect| matches!(effect.what, Effected::Release { .. }))
        .cloned()
        .collect();
    assert_eq!(releases.len(), 1);
    let applied = east.tick(&TickInputs {
        effects: filed(1, &releases, &east),
        ..TickInputs::default()
    });
    assert!(applied.claims_changed.contains(&across));
    assert_eq!(
        claims_of(&east, across),
        [Claim {
            at: in_chunk(head),
            kind: PoiType::Home,
            free: 0
        }]
    );
    // And the east reports the chunk with the lectern free.
    let report = applied.village.expect("the claim changed");
    let (_, records) = report
        .pois
        .iter()
        .find(|(position, _)| *position == across)
        .expect("the chunk is reported");
    assert!(records.contains(&seen(lectern, PoiType::Librarian, false)));
    assert!(records.contains(&seen(head, PoiType::Home, true)));

    // A release that lands after the bed was broken finds no ticket taken there.
    let mut output = TickOutput::default();
    west.release_poi(head, PoiType::Home, &mut output);
    let sent = west.tick(&TickInputs::default());
    let mut scratch = TickOutput::default();
    east.set_block(head, blocks::AIR, &mut scratch);
    assert!(claims_of(&east, across).is_empty(), "broken, no claim");
    let applied = east.tick(&TickInputs {
        effects: filed(2, &sent.effects, &east),
        ..TickInputs::default()
    });
    assert!(!applied.claims_changed.contains(&across));
    assert!(claims_of(&east, across).is_empty());

    // A release of another type than is there does nothing either.
    let mut scratch = TickOutput::default();
    assert!(east.acquire_poi(lectern, &mut scratch));
    let wrong = clustine_world::Effect {
        at: lectern,
        what: Effected::Release {
            kind: PoiType::Mason,
        },
    };
    let applied = east.tick(&TickInputs {
        effects: filed(3, &[wrong], &east),
        ..TickInputs::default()
    });
    assert!(!applied.claims_changed.contains(&across));
    assert_eq!(claims_of(&east, across).len(), 1);
}
