//! Tests of lending at a pinned line (step L3a of `docs/adr/0027-lending-at-a-line.md`):
//! a region asks for the chunks across its line that a circuit's part reaches, keeps
//! what it is lent and gives it back, and a lender waits, refuses or lets go.
//!
//! The line is at block x = 16: the west is pinned to the chunks x < 1 and the east to
//! those from x = 1 on. Each region here is one of the two, told by the test what the
//! store would say.

use clustine_data::items;

use crate::api::LendAnswer;

use super::redstone::{block_at, holding, state};
use super::*;

const WEST_AREA: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EAST_AREA: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};
const THE_WEST: RegionId = RegionId(10);
const THE_EAST: RegionId = RegionId(11);
/// The chunk east of the line at the spawn's row, and the one west of it.
const ACROSS: ChunkPos = ChunkPos::new(1, 0);
const OWN: ChunkPos = ChunkPos::new(0, 0);

fn wire() -> BlockState {
    state("minecraft:redstone_wire[east=none,north=none,power=0,south=none,west=none]")
}

/// A region pinned to `area` that holds `own` and has been told that `theirs` are
/// `holder`'s, with each loaded on a stone floor and `blocks` set.
fn by_the_line(
    area: ChunkArea,
    own: &[ChunkPos],
    theirs: &[ChunkPos],
    holder: RegionId,
    blocks: &[(BlockPos, BlockState)],
) -> Region {
    let holdings = Holdings {
        held: own.to_vec(),
        pinned: vec![area],
        ..Holdings::default()
    };
    let state = knowing_the_edges(EntityIds::block(0).unwrap());
    let mut region = Region::restore(config(), state, holdings);
    let all: Vec<ChunkPos> = own.iter().chain(theirs).copied().collect();
    let output = checked(&mut region, &tickets(all, vec![]));
    assert_eq!(output.claims, theirs);
    let loaded = own
        .iter()
        .map(|position| {
            let mut chunk = floor();
            for (at, block) in blocks.iter().filter(|(at, _)| at.chunk() == *position) {
                let (x, z) = at.in_chunk();
                chunk.set(x, at.y, z, *block);
            }
            (*position, chunk)
        })
        .collect();
    // Theirs are copied, so that the chunks by the line tick.
    checked(
        &mut region,
        &TickInputs {
            foreign: theirs.iter().map(|position| (*position, holder)).collect(),
            chunks_loaded: loaded,
            copy_bases: theirs.iter().map(|position| (*position, floor())).collect(),
            ..TickInputs::default()
        },
    );
    region
}

/// The west, with a player of its own who has walked to the line and holds wire.
fn the_west_with_a_builder() -> Region {
    let own = [
        ChunkPos::new(-1, -1),
        ChunkPos::new(-1, 0),
        ChunkPos::new(-1, 1),
        ChunkPos::new(0, -1),
        OWN,
        ChunkPos::new(0, 1),
    ];
    let theirs = [ChunkPos::new(1, -1), ACROSS, ChunkPos::new(1, 1)];
    let mut region = by_the_line(WEST_AREA, &own, &theirs, THE_EAST, &[]);
    assert!(region.ticks_chunk(OWN));
    checked(&mut region, &changes(vec![join(1)]));
    enter(&mut region);
    checked(&mut region, &moves(vec![walk(1, 12.5, 0.5)]));
    checked(&mut region, &holding(items::REDSTONE));
    region
}

/// Wire placed on the floor at `x`, 0 by the west's builder.
fn wire_at(x: i32) -> TickInputs {
    moves(vec![place(1, x, -61, 0, Face::Top)])
}

#[test]
fn wire_placed_by_the_line_asks_for_the_chunks_across_it_within_two_blocks() {
    let mut region = the_west_with_a_builder();
    let output = checked(&mut region, &wire_at(15));
    let placed = block_at(&region, BlockPos::new(15, -60, 0));
    assert!(placed.is_some_and(|state| clustine_blocks::data::is_block(state, wire())));
    // Two blocks reach z = -2: the row north of the spawn's as well.
    assert_eq!(output.lends, [ChunkPos::new(1, -1), ACROSS]);
    assert_eq!(
        region
            .state()
            .lends_asked
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [ChunkPos::new(1, -1), ACROSS]
    );
    // A part further from the line asks for nothing, and nor does one asked for again.
    let output = checked(&mut region, &wire_at(13));
    assert_eq!(output.lends, []);
}

#[test]
fn an_ask_told_to_wait_is_asked_again_twenty_ticks_on_and_one_not_lendable_is_not() {
    let mut region = the_west_with_a_builder();
    let asked_at = region.tick_number() + 1;
    checked(&mut region, &wire_at(15));
    let told = TickInputs {
        not_lent: vec![(vec![ACROSS], true), (vec![ChunkPos::new(1, -1)], false)],
        ..TickInputs::default()
    };
    checked(&mut region, &told);
    let mut again = Vec::new();
    while region.tick_number() < asked_at + 25 {
        let output = checked(&mut region, &TickInputs::default());
        if !output.lends.is_empty() {
            again.push((output.tick, output.lends));
        }
    }
    assert_eq!(
        again,
        [(
            asked_at + crate::region::lends::ASK_AGAIN_AFTER,
            vec![ACROSS]
        )]
    );
}

#[test]
fn a_lent_chunk_is_kept_loaded_without_a_ticket_and_given_back_once_its_wire_is_gone() {
    let mut region = the_west_with_a_builder();
    checked(&mut region, &wire_at(15));
    // Lent: it is loaded at once, kept while nobody looks at it, and not given back.
    let output = checked(
        &mut region,
        &TickInputs {
            granted: vec![ACROSS],
            ..TickInputs::default()
        },
    );
    assert!(output.chunk_requests.contains(&ACROSS));
    checked(&mut region, &loaded(ACROSS));
    checked(&mut region, &tickets(vec![], vec![ACROSS]));
    for _ in 0..10 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(!output.returns.contains(&ACROSS));
    }
    assert!(region.chunk(ACROSS).is_some());
    assert!(!region.state().lends_asked.contains_key(&ACROSS));
    // The wire broken: the lent chunk goes back, saved as it is, and is dropped in the
    // tick after.
    let broken = moves(vec![numbered(
        1,
        PlayerInput::Dig {
            position: BlockPos::new(15, -60, 0),
            sequence: 2,
        },
    )]);
    let output = checked(&mut region, &broken);
    assert_eq!(
        block_at(&region, BlockPos::new(15, -60, 0)),
        Some(blocks::AIR)
    );
    assert_eq!(output.returns, [ACROSS]);
    assert!(region.chunk(ACROSS).is_some(), "until the next tick");
    checked(&mut region, &TickInputs::default());
    assert!(region.chunk(ACROSS).is_none());
    assert_eq!(region.knowledge(ACROSS), Knowledge::Unknown);
}

/// The east, which holds the chunks east of the line and is told the west's are the
/// west's, with `blocks` set.
fn the_east(blocks: &[(BlockPos, BlockState)]) -> Region {
    let own = [
        ChunkPos::new(1, -1),
        ACROSS,
        ChunkPos::new(1, 1),
        ChunkPos::new(2, -1),
        ChunkPos::new(2, 0),
        ChunkPos::new(2, 1),
    ];
    let theirs = [ChunkPos::new(0, -1), OWN, ChunkPos::new(0, 1)];
    by_the_line(EAST_AREA, &own, &theirs, THE_WEST, blocks)
}

fn asked(lend: u64, chunks: &[ChunkPos]) -> TickInputs {
    TickInputs {
        lends_asked: vec![LendAsked {
            lend,
            chunks: chunks.to_vec(),
            to: THE_WEST,
        }],
        ..TickInputs::default()
    }
}

#[test]
fn a_lender_lets_a_chunk_go_drops_it_a_tick_later_and_believes_the_borrower_until_it_is_back() {
    let mut region = the_east(&[]);
    let output = checked(&mut region, &asked(7, &[ACROSS]));
    assert_eq!(
        output.lend_answers,
        [LendAnswer::Lending {
            lend: 7,
            chunks: vec![ACROSS],
        }]
    );
    assert!(region.chunk(ACROSS).is_some(), "to be saved as it is");
    let output = checked(&mut region, &TickInputs::default());
    assert!(region.chunk(ACROSS).is_none());
    assert!(output.claims.is_empty(), "no claim of a chunk let go of");
    checked(
        &mut region,
        &TickInputs {
            lent: vec![(vec![ACROSS], THE_WEST)],
            ..TickInputs::default()
        },
    );
    // Believed whatever the region wants, until it is pinned back.
    checked(&mut region, &tickets(vec![], vec![ACROSS]));
    checked(&mut region, &TickInputs::default());
    assert_eq!(region.knowledge(ACROSS), Knowledge::Foreign(THE_WEST));
    checked(
        &mut region,
        &TickInputs {
            pinned_back: vec![ACROSS],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.knowledge(ACROSS), Knowledge::Unknown);
}

#[test]
fn a_lend_called_off_is_the_lenders_to_take_up_again() {
    let mut region = the_east(&[]);
    checked(&mut region, &asked(7, &[ACROSS]));
    checked(&mut region, &TickInputs::default());
    let output = checked(
        &mut region,
        &TickInputs {
            lends_called_off: vec![(7, vec![ACROSS])],
            ..TickInputs::default()
        },
    );
    // Its ticket wants it: it is claimed again.
    assert_eq!(output.claims, [ACROSS]);
}

#[test]
fn a_lender_refuses_where_its_own_wire_reaches_the_chunk_from_beside_and_not_from_inside() {
    // Wire two blocks east of the chunk asked for, in the lender's own land.
    let mut region = the_east(&[(BlockPos::new(33, -60, 0), wire())]);
    let output = checked(&mut region, &asked(3, &[ACROSS]));
    assert_eq!(
        output.lend_answers,
        [LendAnswer::Refused {
            lend: 3,
            chunks: vec![ACROSS],
            again: false,
        }]
    );
    // Wire in the chunk asked for goes with it.
    let mut region = the_east(&[(BlockPos::new(17, -60, 0), wire())]);
    let output = checked(&mut region, &asked(4, &[ACROSS]));
    assert!(matches!(
        output.lend_answers[..],
        [LendAnswer::Lending { lend: 4, .. }]
    ));
}

#[test]
fn a_lender_waits_for_what_it_has_not_loaded_and_loads_it_to_see() {
    let own = [ACROSS];
    let theirs = [OWN];
    let mut region = by_the_line(EAST_AREA, &own, &theirs, THE_WEST, &[]);
    // Chunk (2, 0) beside it is the east's own and not loaded.
    let output = checked(&mut region, &asked(5, &[ACROSS]));
    assert_eq!(
        output.lend_answers,
        [LendAnswer::Refused {
            lend: 5,
            chunks: vec![ACROSS],
            again: true,
        }]
    );
    assert!(output.claims.contains(&ChunkPos::new(2, 0)));
}

#[test]
fn an_ask_outlives_a_restore_and_one_for_a_chunk_held_since_is_dropped() {
    let mut region = the_west_with_a_builder();
    checked(&mut region, &wire_at(15));
    let state = region.state();
    let holdings = |held: Vec<ChunkPos>| Holdings {
        held,
        pinned: vec![WEST_AREA],
        ..Holdings::default()
    };
    let again = Region::restore(config(), state.clone(), holdings(vec![OWN]));
    assert_eq!(again.state().lends_asked, state.lends_asked);
    let again = Region::restore(config(), state, holdings(vec![OWN, ACROSS]));
    assert_eq!(
        again
            .state()
            .lends_asked
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [ChunkPos::new(1, -1)]
    );
}

#[test]
fn land_lent_away_is_believed_against_an_action_and_an_arrival_that_name_it() {
    let mut region = the_east(&[]);
    checked(&mut region, &asked(7, &[ACROSS]));
    checked(&mut region, &TickInputs::default());
    checked(
        &mut region,
        &TickInputs {
            lent: vec![(vec![ACROSS], THE_WEST)],
            ..TickInputs::default()
        },
    );
    // What another region's player does there goes on to the borrower, and the belief
    // stays: only the store's word that it is pinned back ends it.
    let action = RemoteAction {
        player: player(9),
        entity: EntityId(90),
        sequence: 1,
        step: RemoteStep::Break {
            position: BlockPos::new(17, -61, 0),
        },
    };
    let output = checked(
        &mut region,
        &TickInputs {
            remote_actions: vec![(REMOTE, action)],
            ..TickInputs::default()
        },
    );
    assert!(
        output.durable.iter().any(|(_, _, entry)| matches!(
            entry,
            Durable::NotMine { holder, .. } if *holder == THE_WEST
        )),
        "{:?}",
        output.durable
    );
    assert_eq!(region.knowledge(ACROSS), Knowledge::Foreign(THE_WEST));
}
