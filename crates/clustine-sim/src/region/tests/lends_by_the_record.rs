//! Tests of lending land at a pinned line, written from `docs/adr/0027-lending-at-a-line.md`
//! (sections 2 to 4) by someone who had not read the code that does it.
//!
//! The borrower is the region pinned to `MIDDLE` (chunks with x 0 and 1), whose player 1
//! stands at `STAND` in chunk (0, 0) with the floors of its six chunks with x 0 and 1 and z
//! -1 to 1 loaded. West of it, chunks with x below 0 are region 0's; east of it, chunks with
//! x from 2 are region 2's. The lender is the region pinned to `WEST` (chunks with x below
//! 0), with the floors of its nine chunks with x -3 to -1 and z -1 to 1 loaded, and the
//! region east of it is region 1.

use clustine_data::BlockClass;

use super::redstone::{block_at, lever, state};
use super::*;
use crate::api::{LendAnswer, LendAsked};

const BORROWER: RegionId = RegionId(1);
const WEST_OF_BORROWER: RegionId = RegionId(0);
const EAST_OF_BORROWER: RegionId = RegionId(2);

/// The chunk west of the line beside the borrower's chunk (0, 0).
const ACROSS: ChunkPos = ChunkPos::new(-1, 0);

/// Where the borrower's players stand: in chunk (0, 0), out of the way of the blocks the
/// tests place, which nothing is placed into where a player stands, and within reach of
/// them.
const STAND: Vec3 = Vec3::new(3.5, -60.0, 5.5);

/// The borrower's own chunks the tests load.
fn borrowers_own() -> Vec<ChunkPos> {
    (0..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A floor with `blocks` set where they fall into the chunk at `position`.
fn floor_at(position: ChunkPos, blocks: &[(BlockPos, BlockState)]) -> Chunk {
    let mut chunk = floor();
    for (at, state) in blocks.iter().filter(|(at, _)| at.chunk() == position) {
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    chunk
}

/// Hands `served` floors as the border copies it wants, as a runner hands in the copies
/// of other regions' chunks, so that its chunks at the line tick: a chunk whose
/// neighbours are neither loaded nor copied does not, and wire is not placed in it.
fn with_copies(served: &mut Served) {
    let copies = served
        .region
        .border_chunks()
        .into_iter()
        .map(|position| (position, floor()))
        .collect();
    served.tick(&TickInputs {
        copy_bases: copies,
        ..TickInputs::default()
    });
}

/// The borrower with the players `numbers` joined, each standing at `STAND`, and its own
/// chunks loaded with `blocks` in them.
fn borrower_of(numbers: &[u128], blocks: &[(BlockPos, BlockState)]) -> Served {
    let mut served = Served::new(MIDDLE, EntityIds::block(0).unwrap());
    assert_eq!(served.id, BORROWER);
    served.tick(&changes(
        numbers.iter().map(|number| join(*number)).collect(),
    ));
    let entering = served.with_answers(TickInputs::default());
    served.tick(&entering);
    let own = borrowers_own();
    served.tick(&tickets(own.clone(), vec![]));
    let loaded = own
        .iter()
        .map(|position| (*position, floor_at(*position, blocks)))
        .collect();
    served.tick(&TickInputs {
        chunks_loaded: loaded,
        ..TickInputs::default()
    });
    with_copies(&mut served);
    assert!(served.region.ticks_chunk(EAST_CHUNK));
    let walks = numbers
        .iter()
        .map(|number| walk(*number, STAND.x, STAND.z))
        .collect();
    let output = served.tick(&moves(walks));
    assert!(output.lends.is_empty(), "nothing was placed yet");
    for position in own {
        assert!(served.region.chunk(position).is_some(), "{position:?}");
    }
    served
}

fn borrower_with(blocks: &[(BlockPos, BlockState)]) -> Served {
    borrower_of(&[1], blocks)
}

fn borrower() -> Served {
    borrower_with(&[])
}

/// Player `number` takes `item` in hand and uses it on the top of the floor at `x`, `z`,
/// which places it on the floor at y = -60.
fn placing(number: u128, item: i32, x: i32, z: i32) -> TickInputs {
    let stack = ItemStack::new(item, 1);
    moves(vec![
        set_slot(number, 0, Some(stack)),
        select(number, 0),
        place(number, x, -61, z, Face::Top),
    ])
}

/// Player 1 places `item` on the floor at `x`, `z` and the tick's output; the block is
/// checked to be there, so that a test of what does not ask does not pass for a
/// placement that failed.
fn place_part(region: &mut Region, item: i32, x: i32, z: i32) -> TickOutput {
    let output = checked(region, &placing(1, item, x, z));
    let placed = block_at(region, BlockPos::new(x, -60, z));
    assert!(
        placed.is_some_and(|placed| placed != blocks::AIR),
        "{item} was placed at {x}, {z}: {placed:?}"
    );
    output
}

/// Player 1 digs the block at `x`, `y`, `z`.
fn digging(x: i32, y: i32, z: i32) -> TickInputs {
    moves(vec![dig(1, x, y, z, 1)])
}

/// The asks for lends the region keeps, ascending.
fn kept_asks(region: &Region) -> Vec<ChunkPos> {
    region.state().lends_asked.keys().copied().collect()
}

/// The borrower after player 1 placed wire at (1, -60, 2), two blocks from `ACROSS`, and
/// the tick of that ask.
fn borrower_asking() -> (Served, u64) {
    let mut served = borrower();
    let output = served.tick(&placing(1, items::REDSTONE, 1, 2));
    assert_eq!(output.lends, [ACROSS]);
    (served, output.tick)
}

/// Has the store grant `position` to the borrower as a lend, and storage deliver it with
/// `blocks`. The store's table of the test notes the grant, so that the return is checked
/// against it.
fn lend_to(served: &mut Served, position: ChunkPos, blocks: &[(BlockPos, BlockState)]) {
    served.grants.granted.insert(position, served.id);
    let output = served.tick(&answers(vec![position], vec![]));
    assert_eq!(served.region.knowledge(position), Knowledge::Held);
    assert!(
        output.chunk_requests.contains(&position),
        "{:?}",
        output.chunk_requests
    );
    // Its copy is withdrawn as it loads, as the runner does with a chunk loaded as the
    // region's own.
    served.tick(&TickInputs {
        chunks_loaded: vec![(position, floor_at(position, blocks))],
        copies_withdrawn: vec![position],
        ..TickInputs::default()
    });
    assert!(served.region.chunk(position).is_some());
}

/// The borrower with `ACROSS` lent to it for wire at (1, -60, 2), and the viewer's ticket
/// it had on `ACROSS` from the start released, so that only the lend keeps it.
fn borrower_with_the_lend() -> Served {
    let (mut served, _) = borrower_asking();
    lend_to(&mut served, ACROSS, &[]);
    served.tick(&tickets(vec![], vec![ACROSS]));
    served
}

/// Section 2: a circuit's part placed by a player of the region in its own pinned land,
/// within two blocks of a chunk it does not hold, asks for that chunk; the ask is kept in
/// the region's state and named in the tick's delta.
#[test]
fn wire_placed_two_blocks_from_land_across_the_line_asks_for_it_and_keeps_the_ask() {
    let mut served = borrower();
    let output = place_part(&mut served.region, items::REDSTONE, 1, 2);
    assert_eq!(output.lends, [ACROSS]);
    assert_eq!(
        served.region.state().lends_asked,
        BTreeMap::from([(ACROSS, output.tick)])
    );
    assert_eq!(output.delta.lends_asked, [(ACROSS, Some(output.tick))]);
    assert!(!output.delta.changes_only_the_tick());
    // The next tick asks nothing again and changes nothing of the ask.
    let output = idle(&mut served.region);
    assert!(output.lends.is_empty());
    assert!(output.delta.lends_asked.is_empty());
    assert_eq!(kept_asks(&served.region), [ACROSS]);
}

/// Section 2: the reach is two blocks, so wire three blocks from the line asks nothing.
#[test]
fn wire_placed_three_blocks_from_the_line_asks_nothing() {
    let mut served = borrower();
    let output = place_part(&mut served.region, items::REDSTONE, 2, 2);
    assert!(output.lends.is_empty(), "{:?}", output.lends);
    assert!(kept_asks(&served.region).is_empty());
}

/// Section 2: only chunks the region does not hold and is not pinned to are asked for. A
/// part at the corner of chunk (0, 0) reaches both chunks across the line and the
/// region's own chunk (0, 1), which it does not ask for.
#[test]
fn a_part_at_a_corner_asks_for_both_chunks_across_and_not_for_its_own() {
    let mut served = borrower();
    served.tick(&moves(vec![walk(1, 1.5, 13.5)]));
    let output = place_part(&mut served.region, items::REDSTONE, 0, 15);
    assert_eq!(output.lends, [ACROSS, ChunkPos::new(-1, 1)]);
    assert_eq!(kept_asks(&served.region), [ACROSS, ChunkPos::new(-1, 1)]);
}

/// Section 2: every kind of the record's circuit's parts asks, and a state the block data
/// says is a signal source (a daylight detector).
#[test]
fn every_circuits_part_of_the_record_asks_when_placed_at_the_line() {
    let base = borrower();
    let parts = [
        ("wire", items::REDSTONE),
        ("torch", items::REDSTONE_TORCH),
        ("repeater", items::REPEATER),
        ("comparator", items::COMPARATOR),
        ("observer", items::OBSERVER),
        ("piston", items::PISTON),
        ("sticky piston", items::STICKY_PISTON),
        ("lever", items::LEVER),
        ("button", items::STONE_BUTTON),
        ("plate", items::STONE_PRESSURE_PLATE),
        ("redstone block", items::REDSTONE_BLOCK),
        ("target", items::TARGET),
        ("note block", items::NOTE_BLOCK),
        ("copper bulb", items::COPPER_BULB),
        ("lamp", items::REDSTONE_LAMP),
        ("daylight detector", items::DAYLIGHT_DETECTOR),
    ];
    for (name, item) in parts {
        let mut region = base.region.clone();
        let output = place_part(&mut region, item, 1, 2);
        assert_eq!(output.lends, [ACROSS], "a {name} asks");
    }
}

/// Section 2: what only reads a signal does not ask, nor does a block that has nothing to
/// do with redstone.
#[test]
fn doors_trapdoors_gates_rails_and_stone_placed_at_the_line_ask_nothing() {
    let base = borrower();
    let others = [
        ("door", items::OAK_DOOR),
        ("trapdoor", items::OAK_TRAPDOOR),
        ("gate", items::OAK_FENCE_GATE),
        ("rail", items::RAIL),
        ("powered rail", items::POWERED_RAIL),
        ("stone", items::STONE),
    ];
    for (name, item) in others {
        let mut region = base.region.clone();
        let output = place_part(&mut region, item, 1, 2);
        assert!(output.lends.is_empty(), "a {name} asks {:?}", output.lends);
        assert!(kept_asks(&region).is_empty(), "a {name}");
    }
}

/// Section 2: a placement by another region's player through the edge, a remote action,
/// asks too.
#[test]
fn a_remote_placement_of_a_part_at_the_line_asks_too() {
    let mut served = borrower();
    let placer = Vec3::new(-2.5, -60.0, 5.5);
    let step = place_against(1, -61, 5, Face::Top, blocks::REDSTONE_WIRE, placer);
    let output = served.tick(&remotely(vec![remote(2, 1, step)]));
    assert_eq!(outcomes(&output), [done(2, 1)]);
    assert!(
        block_at(&served.region, BlockPos::new(1, -60, 5)).is_some_and(|wire| wire != blocks::AIR)
    );
    assert_eq!(output.lends, [ACROSS]);
    // A remote placement of stone asks nothing.
    let step = place_against(0, -61, 7, Face::Top, blocks::STONE, placer);
    let output = served.tick(&remotely(vec![remote(2, 2, step)]));
    assert_eq!(outcomes(&output), [done(2, 2)]);
    assert!(output.lends.is_empty());
}

/// Section 2: for a piston the reach is its structure as the resolver finds it at
/// placing. A piston seven blocks from the line facing it, with six blocks before it that
/// it would push into the chunk across, asks for that chunk; the same piston with nothing
/// before it is too far to ask.
#[test]
fn a_piston_whose_structure_reaches_across_asks_and_one_without_does_not() {
    let piston = state("minecraft:piston[facing=west,extended=false]");
    let placer = Vec3::new(8.5, -60.0, 4.5);
    let step = || place_against(6, -61, 2, Face::Top, piston, placer);
    let stones: Vec<_> = (0..=5)
        .map(|x| (BlockPos::new(x, -60, 2), blocks::STONE))
        .collect();

    let mut served = borrower_with(&stones);
    let output = served.tick(&remotely(vec![remote(2, 1, step())]));
    assert_eq!(outcomes(&output), [done(2, 1)]);
    assert_eq!(
        block_at(&served.region, BlockPos::new(6, -60, 2)),
        Some(piston)
    );
    assert_eq!(output.lends, [ACROSS]);

    let mut served = borrower();
    let output = served.tick(&remotely(vec![remote(2, 1, step())]));
    assert_eq!(
        block_at(&served.region, BlockPos::new(6, -60, 2)),
        Some(piston)
    );
    assert!(output.lends.is_empty(), "{:?}", output.lends);
}

/// Section 2: nothing but a placement asks. A lever at the line that was there when its
/// chunk loaded asks nothing, nor does pulling it, which powers wire beside it, nor does
/// digging it.
#[test]
fn a_part_that_was_there_pulled_or_dug_asks_nothing() {
    let at = BlockPos::new(1, -60, 2);
    let mut served = borrower_with(&[
        (at, lever(false)),
        (BlockPos::new(1, -60, 3), blocks::REDSTONE_WIRE),
    ]);
    // Stone is in hand: using it on the lever pulls the lever.
    let output = served.tick(&moves(vec![place(1, 1, -60, 2, Face::Top)]));
    assert_eq!(block_at(&served.region, at), Some(lever(true)));
    assert!(output.lends.is_empty(), "{:?}", output.lends);
    let output = served.tick(&digging(1, -60, 2));
    assert_eq!(block_at(&served.region, at), Some(blocks::AIR));
    assert!(output.lends.is_empty(), "{:?}", output.lends);
    assert!(kept_asks(&served.region).is_empty());
}

/// Section 2, the wait: an ask answered `NotLent` with `again` is kept, and asked again
/// twenty ticks after it was asked last, not before.
#[test]
fn an_ask_answered_wait_is_kept_and_asked_again_twenty_ticks_after() {
    let (mut served, asked) = borrower_asking();
    let output = served.tick(&TickInputs {
        not_lent: vec![(vec![ACROSS], true)],
        ..TickInputs::default()
    });
    assert!(output.lends.is_empty());
    assert_eq!(kept_asks(&served.region), [ACROSS]);
    let again = (0..60)
        .map(|_| idle(&mut served.region))
        .find(|output| !output.lends.is_empty())
        .expect("asked again");
    assert_eq!(again.lends, [ACROSS]);
    assert_eq!(again.tick, asked + 20);
    assert_eq!(again.delta.lends_asked, [(ACROSS, Some(again.tick))]);
    // And once more twenty ticks after that, for as long as it waits.
    served.tick(&TickInputs {
        not_lent: vec![(vec![ACROSS], true)],
        ..TickInputs::default()
    });
    let third = (0..60)
        .map(|_| idle(&mut served.region))
        .find(|output| !output.lends.is_empty())
        .expect("asked a third time");
    assert_eq!(third.tick, again.tick + 20);
}

/// Section 2: a call-off keeps the ask, which is asked again twenty ticks after it was
/// asked last.
#[test]
fn an_ask_whose_lend_is_called_off_is_kept_and_asked_again() {
    let (mut served, asked) = borrower_asking();
    let output = served.tick(&TickInputs {
        lends_called_off: vec![(7, vec![ACROSS])],
        ..TickInputs::default()
    });
    assert!(output.lends.is_empty());
    assert_eq!(kept_asks(&served.region), [ACROSS]);
    let again = (0..60)
        .map(|_| idle(&mut served.region))
        .find(|output| !output.lends.is_empty())
        .expect("asked again");
    assert_eq!(again.lends, [ACROSS]);
    assert_eq!(again.tick, asked + 20);
}

/// The borrower after its ask for `ACROSS` was answered not lendable.
fn borrower_refused() -> Served {
    let (mut served, _) = borrower_asking();
    let output = served.tick(&TickInputs {
        not_lent: vec![(vec![ACROSS], false)],
        ..TickInputs::default()
    });
    assert_eq!(output.delta.lends_asked, [(ACROSS, None)]);
    assert!(kept_asks(&served.region).is_empty());
    served
}

/// Section 2: not lendable ends the ask, and the region does not ask for the chunk again,
/// neither by time nor for another part placed within reach of it.
#[test]
fn an_ask_answered_not_lendable_ends_and_is_not_made_again() {
    let mut served = borrower_refused();
    for _ in 0..60 {
        let output = idle(&mut served.region);
        assert!(output.lends.is_empty(), "tick {}", output.tick);
    }
    let output = place_part(&mut served.region, items::REDSTONE, 0, 6);
    assert!(output.lends.is_empty(), "{:?}", output.lends);
    assert!(kept_asks(&served.region).is_empty());
}

/// Section 2: the answer not lendable is kept for a minute (the record's "until
/// `PinnedBack`" could not apply: a borrower is told `PinnedBack` only of its own land).
#[test]
fn a_minute_after_not_lendable_a_part_placed_at_the_line_asks_again() {
    let mut served = borrower_refused();
    // The borrower is never told when what stands in the lender's land changes, so the
    // word lapses after a minute ("Found while building", L5).
    for _ in 0..1_200 {
        checked(&mut served.region, &TickInputs::default());
    }
    let output = place_part(&mut served.region, items::REDSTONE, 0, 6);
    assert_eq!(output.lends, [ACROSS]);
}

/// Section 2: the answer not lendable is kept until a merge of the region's own.
#[test]
fn after_a_merge_a_part_placed_at_the_line_asks_again() {
    let mut served = borrower_refused();
    let east = RegionState::new(EntityIds::block(2).unwrap());
    let merged = served.region.absorb(EAST_OF_BORROWER, &east);
    let east_area = ChunkArea {
        min_x: Some(2),
        max_x: None,
    };
    served
        .region
        .take_absorbed(merged, &[], &[east_area], &[], &[]);
    // The merge left nothing loaded: the home chunk comes back by itself, and chunk
    // (0, 0) is all the placement needs.
    let output = checked(&mut served.region, &TickInputs::default());
    assert!(output.chunk_requests.contains(&EAST_CHUNK));
    checked(&mut served.region, &loaded_floor(EAST_CHUNK));
    assert!(served.region.chunk(EAST_CHUNK).is_some());
    let output = place_part(&mut served.region, items::REDSTONE_TORCH, 0, 6);
    assert_eq!(output.lends, [ACROSS]);
}

fn loaded_floor(position: ChunkPos) -> TickInputs {
    TickInputs {
        chunks_loaded: vec![(position, floor())],
        ..TickInputs::default()
    }
}

/// Section 2: a grant ends the ask; the chunk is the region's.
#[test]
fn a_grant_ends_the_ask() {
    let (mut served, _) = borrower_asking();
    served.grants.granted.insert(ACROSS, BORROWER);
    let output = served.tick(&answers(vec![ACROSS], vec![]));
    assert_eq!(output.delta.lends_asked, [(ACROSS, None)]);
    assert!(kept_asks(&served.region).is_empty());
    assert_eq!(served.region.knowledge(ACROSS), Knowledge::Held);
}

/// Section 2: a part placed in lent land asks for nothing, though it is within two blocks
/// of a chunk the region does not hold: lends do not creep.
#[test]
fn a_part_placed_in_lent_land_asks_for_nothing() {
    let mut served = borrower_with_the_lend();
    // Another region's player places wire at the western edge of the lent chunk, a block
    // from chunk (-2, 0), which is region 0's.
    let placer = Vec3::new(-12.5, -60.0, 4.5);
    let step = place_against(-16, -61, 2, Face::Top, blocks::REDSTONE_WIRE, placer);
    let output = served.tick(&remotely(vec![remote(2, 1, step)]));
    assert_eq!(outcomes(&output), [done(2, 1)]);
    assert!(
        block_at(&served.region, BlockPos::new(-16, -60, 2))
            .is_some_and(|wire| wire != blocks::AIR)
    );
    assert!(output.lends.is_empty(), "{:?}", output.lends);
    assert!(kept_asks(&served.region).is_empty());
}

/// Section 2: a restore drops the asks for chunks the region holds, and keeps the others.
#[test]
fn a_restore_drops_the_asks_for_chunks_the_region_holds() {
    let mut state = knowing_the_edges(EntityIds::block(0).unwrap());
    for position in [ACROSS, ChunkPos::new(-1, 1), ChunkPos::new(2, 0)] {
        state.lends_asked.insert(position, 5);
    }
    let holdings = Holdings {
        held: vec![EAST_CHUNK, ACROSS],
        pinned: vec![MIDDLE],
        lent: vec![ACROSS],
        lent_away: Vec::new(),
    };
    let mut region = Region::restore(config(), state, holdings);
    assert_eq!(
        kept_asks(&region),
        [ChunkPos::new(-1, 1), ChunkPos::new(2, 0)]
    );
    idle(&mut region);
    assert_eq!(
        kept_asks(&region),
        [ChunkPos::new(-1, 1), ChunkPos::new(2, 0)]
    );
}

/// Section 2: a split's part, which is not pinned, asks nothing and drops its asks.
#[test]
fn a_splits_part_has_no_asks_and_asks_nothing() {
    let mut served = borrower_of(&[1, 2], &[]);
    served.tick(&moves(vec![walk(2, 1.5, 40.5)]));
    let output = served.tick(&placing(1, items::REDSTONE, 1, 2));
    assert_eq!(output.lends, [ACROSS]);
    let gone = ChunkPos::new(0, 2);
    let splitting = served
        .region
        .split(&[gone], RegionId(7), &[])
        .expect("player 2 goes");
    assert!(splitting.chunks.contains(&gone));
    assert!(splitting.part.lends_asked.is_empty());
    let (_, part) = served.region.take_split(splitting, &[]);
    let mut part = part.region;
    assert!(part.state().lends_asked.is_empty());
    // The part loads the chunk player 2 stands in, a chunk at the line, and player 2
    // places wire a block from the land across it.
    let output = checked(&mut part, &tickets(vec![gone], vec![]));
    assert!(output.chunk_requests.contains(&gone));
    checked(&mut part, &loaded_floor(gone));
    let output = checked(&mut part, &placing(2, items::REDSTONE_TORCH, 0, 40));
    assert!(block_at(&part, BlockPos::new(0, -60, 40)).is_some_and(|wire| wire != blocks::AIR));
    assert!(output.lends.is_empty(), "{:?}", output.lends);
    assert!(part.state().lends_asked.is_empty());
}

/// Section 2: a merge's survivor keeps the asks of both that are for chunks it does not
/// hold and that are not in its own pinned area.
#[test]
fn a_merges_survivor_keeps_the_asks_of_both_it_does_not_hold_and_is_not_pinned_to() {
    let mut served = borrower();
    served.tick(&placing(1, items::REDSTONE, 1, 2));
    // Wire a block from region 2's land asks for chunk (2, 0), which the merge makes the
    // survivor's own pinned land.
    served.tick(&moves(vec![walk(1, 29.5, 2.5)]));
    let output = place_part(&mut served.region, items::REDSTONE, 31, 2);
    assert_eq!(output.lends, [ChunkPos::new(2, 0)]);
    assert_eq!(kept_asks(&served.region), [ACROSS, ChunkPos::new(2, 0)]);

    // The absorbed region had asked for chunk (1, 0), the survivor's own; for (-1, 5),
    // which it holds by a lend that comes with the merge; and for (-1, 7).
    let mut east = RegionState::new(EntityIds::block(2).unwrap());
    for position in [
        ChunkPos::new(1, 0),
        ChunkPos::new(-1, 5),
        ChunkPos::new(-1, 7),
    ] {
        east.lends_asked.insert(position, 3);
    }
    let merged = served.region.absorb(EAST_OF_BORROWER, &east);
    let east_area = ChunkArea {
        min_x: Some(2),
        max_x: None,
    };
    let lent = [ChunkPos::new(-1, 5)];
    served
        .region
        .take_absorbed(merged, &lent, &[east_area], &lent, &[]);
    assert_eq!(kept_asks(&served.region), [ACROSS, ChunkPos::new(-1, 7)]);
}

/// The lender with `blocks` in the floors of its nine chunks near the line, each with a
/// viewer's ticket.
fn lender_with(blocks: &[(BlockPos, BlockState)]) -> Served {
    let mut served = Served::new(WEST, EntityIds::block(1).unwrap());
    assert_eq!(served.id, WEST_REGION);
    let own: Vec<ChunkPos> = (-3..=-1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    served.tick(&tickets(own.clone(), vec![]));
    let loaded = own
        .iter()
        .map(|position| (*position, floor_at(*position, blocks)))
        .collect();
    served.tick(&TickInputs {
        chunks_loaded: loaded,
        ..TickInputs::default()
    });
    with_copies(&mut served);
    for position in own {
        assert!(served.region.chunk(position).is_some(), "{position:?}");
    }
    assert!(served.region.ticks_chunk(ACROSS));
    served
}

fn lender() -> Served {
    lender_with(&[])
}

/// The store's ask of the lend numbered `lend` of `chunks` to region 1.
fn asked(lend: u64, chunks: &[ChunkPos]) -> TickInputs {
    TickInputs {
        lends_asked: vec![LendAsked {
            lend,
            chunks: chunks.to_vec(),
            to: EAST_REGION,
        }],
        ..TickInputs::default()
    }
}

fn wait(lend: u64, chunks: &[ChunkPos]) -> LendAnswer {
    LendAnswer::Refused {
        lend,
        chunks: chunks.to_vec(),
        again: true,
    }
}

fn not_lendable(lend: u64, chunks: &[ChunkPos]) -> LendAnswer {
    LendAnswer::Refused {
        lend,
        chunks: chunks.to_vec(),
        again: false,
    }
}

fn lending(lend: u64, chunks: &[ChunkPos]) -> LendAnswer {
    LendAnswer::Lending {
        lend,
        chunks: chunks.to_vec(),
    }
}

/// Section 3, step 2: the lender decides at the end of the tick it is asked in, and with
/// nothing in the way lets the chunk go.
#[test]
fn a_lender_with_nothing_in_the_way_lends_in_the_tick_it_is_asked() {
    let mut served = lender();
    let output = served.tick(&asked(1, &[ACROSS]));
    assert_eq!(output.lend_answers, [lending(1, &[ACROSS])]);
}

/// Section 3, step 2: where the chunk asked for and its own chunks beside it are not
/// loaded, the lender waits and loads them; a load still unanswered is waited for again;
/// once they are loaded it lends.
#[test]
fn a_lender_waits_for_the_chunk_and_its_own_beside_and_loads_them() {
    let mut served = Served::new(WEST, EntityIds::block(1).unwrap());
    let first = served.tick(&asked(1, &[ACROSS]));
    assert_eq!(first.lend_answers, [wait(1, &[ACROSS])]);
    // The loads go out with the tick of the answer or the next; the record does not say
    // which. Asked again meanwhile, with nothing delivered, it waits again.
    let second = served.tick(&asked(2, &[ACROSS]));
    assert_eq!(second.lend_answers, [wait(2, &[ACROSS])]);
    let requested: Vec<ChunkPos> = first
        .chunk_requests
        .iter()
        .chain(&second.chunk_requests)
        .copied()
        .collect();
    let wanted = [
        ChunkPos::new(-2, -1),
        ChunkPos::new(-2, 0),
        ChunkPos::new(-2, 1),
        ChunkPos::new(-1, -1),
        ACROSS,
        ChunkPos::new(-1, 1),
    ];
    for position in wanted {
        assert!(
            requested.contains(&position),
            "{position:?} in {requested:?}"
        );
    }
    let loaded = wanted.iter().map(|position| (*position, floor())).collect();
    served.tick(&TickInputs {
        chunks_loaded: loaded,
        ..TickInputs::default()
    });
    for position in wanted {
        assert!(served.region.chunk(position).is_some(), "{position:?}");
    }
    let output = served.tick(&asked(3, &[ACROSS]));
    assert_eq!(output.lend_answers, [lending(3, &[ACROSS])]);
}

/// Section 3, step 2: one of the lender's players standing in the chunk makes it wait;
/// once they step back into another chunk it lends.
#[test]
fn a_lender_waits_while_its_player_stands_in_the_chunk() {
    let mut served = lender();
    served.tick(&changes(vec![arrive(1, &transfer(1, 1, -8.0, 0))]));
    assert_eq!(served.region.crowds(), [(ACROSS, 1)]);
    let output = served.tick(&asked(1, &[ACROSS]));
    assert_eq!(output.lend_answers, [wait(1, &[ACROSS])]);
    served.tick(&moves(vec![walk(1, -24.5, 8.5)]));
    assert_eq!(served.region.crowds(), [(ChunkPos::new(-2, 0), 1)]);
    let output = served.tick(&asked(2, &[ACROSS]));
    assert_eq!(output.lend_answers, [lending(2, &[ACROSS])]);
}

/// Section 3, step 2: a moving piston in the chunk makes the lender wait; once the move is
/// done it lends. The piston and what powers it lie well inside the chunk asked for, so
/// they do not count as its circuit outside.
#[test]
fn a_lender_waits_while_a_piston_moves_in_the_chunk() {
    let chunk = ChunkPos::new(-2, 0);
    let piston = BlockPos::new(-24, -60, 8);
    let head = BlockPos::new(-23, -60, 8);
    let mut served = lender_with(&[(
        piston,
        state("minecraft:piston[facing=east,extended=false]"),
    )]);
    assert!(served.region.ticks_chunk(chunk));
    // Another region's player puts a redstone block beside it.
    let placer = Vec3::new(-27.5, -60.0, 10.5);
    let step = place_against(-25, -61, 8, Face::Top, blocks::REDSTONE_BLOCK, placer);
    let output = served.tick(&remotely(vec![remote(2, 1, step)]));
    assert_eq!(outcomes(&output), [done(2, 1)]);
    let moving = |region: &Region| {
        block_at(region, head).is_some_and(|state| {
            clustine_blocks::data::is_class(state, BlockClass::MovingPistonBlock)
        })
    };
    assert!(
        moving(&served.region),
        "{:?}",
        block_at(&served.region, head)
    );
    let output = served.tick(&asked(1, &[chunk]));
    assert_eq!(output.lend_answers, [wait(1, &[chunk])]);
    for _ in 0..10 {
        if !moving(&served.region) {
            break;
        }
        idle(&mut served.region);
    }
    assert!(!moving(&served.region));
    let output = served.tick(&asked(2, &[chunk]));
    assert_eq!(output.lend_answers, [lending(2, &[chunk])]);
}

/// Section 3, step 2: the chunk a player enters the world in is waited for, never lent.
#[test]
fn a_lender_waits_for_its_home_chunk() {
    let mut served = borrower();
    let output = served.tick(&TickInputs {
        lends_asked: vec![LendAsked {
            lend: 1,
            chunks: vec![EAST_CHUNK],
            to: WEST_OF_BORROWER,
        }],
        ..TickInputs::default()
    });
    assert_eq!(output.lend_answers, [wait(1, &[EAST_CHUNK])]);
    // The chunk east of it, with its own chunks beside loaded too, is lent.
    let east = ChunkPos::new(1, 0);
    let output = served.tick(&TickInputs {
        lends_asked: vec![LendAsked {
            lend: 2,
            chunks: vec![east],
            to: WEST_OF_BORROWER,
        }],
        ..TickInputs::default()
    });
    assert_eq!(output.lend_answers, [lending(2, &[east])]);
}

/// Section 3, step 2: a circuit's part of the lender's own outside the chunks asked for
/// and within two blocks of them makes the chunk not lendable; three blocks off it does
/// not.
#[test]
fn a_lenders_own_part_within_two_blocks_outside_makes_the_chunk_not_lendable() {
    let at = |x: i32, z: i32| BlockPos::new(x, -60, z);
    for (place, lendable) in [
        // In chunk (-2, 0), one, two and three blocks west of the chunk asked for.
        (at(-17, 8), false),
        (at(-18, 8), false),
        (at(-19, 8), true),
        // In chunk (-1, -1), a block north of it.
        (at(-8, -1), false),
        // In chunk (-1, 1), two and three blocks south of it.
        (at(-8, 17), false),
        (at(-8, 18), true),
    ] {
        let mut served = lender_with(&[(place, lever(false))]);
        let output = served.tick(&asked(1, &[ACROSS]));
        let answer = if lendable {
            lending(1, &[ACROSS])
        } else {
            not_lendable(1, &[ACROSS])
        };
        assert_eq!(output.lend_answers, [answer], "a lever at {place:?}");
    }
}

/// Section 3, step 2: parts inside the chunks asked for go with the lend and do not
/// count, at their edge too.
#[test]
fn a_lenders_parts_inside_the_chunks_asked_for_do_not_count() {
    let mut served = lender_with(&[
        (BlockPos::new(-16, -60, 8), lever(false)),
        (BlockPos::new(-15, -60, 8), blocks::REDSTONE_WIRE),
        (BlockPos::new(-8, -60, 0), blocks::REDSTONE_WIRE),
        (BlockPos::new(-8, -60, 15), blocks::REDSTONE_TORCH),
    ]);
    let output = served.tick(&asked(1, &[ACROSS]));
    assert_eq!(output.lend_answers, [lending(1, &[ACROSS])]);

    // Two chunks asked together: a part at the edge of one beside the other counts for
    // neither.
    let both = [ChunkPos::new(-2, 0), ACROSS];
    let mut served = lender_with(&[(BlockPos::new(-17, -60, 8), lever(false))]);
    let output = served.tick(&asked(1, &both));
    assert_eq!(output.lend_answers, [lending(1, &both)]);
}

/// Section 3, step 2: a piston's reach is its structure. A piston six blocks from the
/// chunk asked for, with five blocks before it that it would push into that chunk, makes
/// it not lendable; the piston alone does not.
#[test]
fn a_lenders_piston_whose_structure_reaches_the_chunk_makes_it_not_lendable() {
    let piston = (
        BlockPos::new(-22, -60, 8),
        state("minecraft:piston[facing=east,extended=false]"),
    );
    let mut blocks = vec![piston];
    blocks.extend((-21..=-17).map(|x| (BlockPos::new(x, -60, 8), blocks::STONE)));
    let mut served = lender_with(&blocks);
    let output = served.tick(&asked(1, &[ACROSS]));
    assert_eq!(output.lend_answers, [not_lendable(1, &[ACROSS])]);

    let mut served = lender_with(&[piston]);
    let output = served.tick(&asked(1, &[ACROSS]));
    assert_eq!(output.lend_answers, [lending(1, &[ACROSS])]);
}

/// The lender after it let `ACROSS` go for the lend numbered 1, with the viewer's ticket
/// on it kept, and the output of that tick.
fn lender_lending() -> (Served, TickOutput) {
    let mut served = lender();
    let output = served.tick(&asked(1, &[ACROSS]));
    assert_eq!(output.lend_answers, [lending(1, &[ACROSS])]);
    (served, output)
}

/// Section 3, step 2: a chunk let go of for a lend is still loaded at the end of that
/// tick, to be saved as it is, and dropped at the start of the next; it is neither
/// claimed nor loaded again before the lend's end, its tickets and a `Loaded` answer
/// notwithstanding.
#[test]
fn a_chunk_let_go_for_a_lend_is_dropped_next_tick_and_not_taken_up_again() {
    let (mut served, _) = lender_lending();
    assert!(served.region.chunk(ACROSS).is_some());
    let output = served.tick(&TickInputs::default());
    assert!(served.region.chunk(ACROSS).is_none());
    assert!(!output.claims.contains(&ACROSS));
    assert!(!output.chunk_requests.contains(&ACROSS));
    // Another ticket on it, and storage delivering it, change nothing.
    let output = served.tick(&TickInputs {
        chunks_loaded: vec![(ACROSS, floor())],
        ..tickets_of(Ticket::Guest, &[ACROSS], &[])
    });
    assert!(!output.claims.contains(&ACROSS));
    assert!(!output.chunk_requests.contains(&ACROSS));
    assert!(served.region.chunk(ACROSS).is_none());
    for _ in 0..30 {
        let output = idle(&mut served.region);
        assert!(!output.claims.contains(&ACROSS));
        assert!(!output.chunk_requests.contains(&ACROSS));
    }
    assert!(served.region.chunk(ACROSS).is_none());
}

/// The lender after `ACROSS` was lent to region 1, the store having told it so.
fn lender_lent() -> Served {
    let (mut served, _) = lender_lending();
    served.tick(&TickInputs {
        lent: vec![(vec![ACROSS], EAST_REGION)],
        ..TickInputs::default()
    });
    assert_eq!(
        served.region.knowledge(ACROSS),
        Knowledge::Foreign(EAST_REGION)
    );
    served
}

/// Section 3, step 4: on `Lent` the lender believes the chunk the borrower's, whatever
/// its tickets want of it, until `PinnedBack`; then it claims the chunk again as its
/// tickets want it.
#[test]
fn a_lent_chunk_is_believed_the_borrowers_whatever_is_wanted_until_it_is_pinned_back() {
    let mut served = lender_lent();
    // Without any ticket on it,
    served.tick(&tickets(vec![], vec![ACROSS]));
    assert_eq!(
        served.region.knowledge(ACROSS),
        Knowledge::Foreign(EAST_REGION)
    );
    // and with a ticket on it again, it is not claimed.
    served.tick(&tickets(vec![ACROSS], vec![]));
    for _ in 0..30 {
        let output = idle(&mut served.region);
        assert!(!output.claims.contains(&ACROSS));
        assert_eq!(
            served.region.knowledge(ACROSS),
            Knowledge::Foreign(EAST_REGION)
        );
    }
    let output = served.tick(&TickInputs {
        pinned_back: vec![ACROSS],
        ..TickInputs::default()
    });
    assert_ne!(
        served.region.knowledge(ACROSS),
        Knowledge::Foreign(EAST_REGION)
    );
    let claimed = output.claims.contains(&ACROSS)
        || served.tick(&TickInputs::default()).claims.contains(&ACROSS);
    assert!(claimed, "claimed again for its tickets");
}

/// Section 3, step 4: the lender trusts its belief until `PinnedBack`. A pinned region
/// otherwise doubts a belief in its own area at an arrival for the chunk (the record's
/// Context, and its first review's twelfth finding); the lend's belief it keeps.
#[test]
fn a_lenders_belief_in_a_lent_chunk_stands_when_a_player_arrives_for_it() {
    let mut served = lender_lent();
    served.tick(&changes(vec![arrive(1, &transfer(1, 1, -8.0, 0))]));
    assert_eq!(
        served.region.knowledge(ACROSS),
        Knowledge::Foreign(EAST_REGION)
    );
}

/// Section 3, step 4: the same for an action of another region's player on a block of the
/// lent chunk, which the lender passes on to the borrower.
#[test]
fn a_lenders_belief_in_a_lent_chunk_stands_when_an_action_comes_for_it() {
    let mut served = lender_lent();
    let action = remote(2, 1, break_at(-8, -61, 3));
    let output = served.tick(&remotely(vec![action.clone()]));
    assert_eq!(
        served.region.knowledge(ACROSS),
        Knowledge::Foreign(EAST_REGION)
    );
    assert_eq!(
        outcomes(&output),
        [RemoteOutcome::NotMine(action, EAST_REGION)]
    );
}

/// Answers what `served` asks of storage with floors until the chunk at `position` is
/// loaded, for at most a few ticks.
fn serve_until_loaded(served: &mut Served, position: ChunkPos, mut output: TickOutput) {
    for _ in 0..5 {
        if served.region.chunk(position).is_some() {
            return;
        }
        let loaded = output
            .chunk_requests
            .iter()
            .map(|requested| (*requested, floor()))
            .collect();
        let inputs = served.with_answers(TickInputs {
            chunks_loaded: loaded,
            ..TickInputs::default()
        });
        output = served.tick(&inputs);
    }
    assert!(
        served.region.chunk(position).is_some(),
        "{position:?} loaded"
    );
}

/// Section 3, call-offs: the lender takes the chunk up again as its tickets want it.
#[test]
fn a_call_off_has_the_lender_take_the_chunk_up_again() {
    let (mut served, _) = lender_lending();
    served.tick(&TickInputs::default());
    assert!(served.region.chunk(ACROSS).is_none());
    let output = served.tick(&TickInputs {
        lends_called_off: vec![(1, vec![ACROSS])],
        ..TickInputs::default()
    });
    assert!(
        output.claims.contains(&ACROSS) || output.chunk_requests.contains(&ACROSS),
        "taken up: {output:?}"
    );
    serve_until_loaded(&mut served, ACROSS, output);
    assert_eq!(served.region.knowledge(ACROSS), Knowledge::Held);
}

/// Section 3, step 1: a message with another lend's number is passed over.
#[test]
fn a_call_off_naming_another_lends_number_changes_nothing() {
    let (mut served, _) = lender_lending();
    served.tick(&TickInputs::default());
    let output = served.tick(&TickInputs {
        lends_called_off: vec![(2, vec![ACROSS])],
        ..TickInputs::default()
    });
    assert!(!output.claims.contains(&ACROSS));
    assert!(!output.chunk_requests.contains(&ACROSS));
    for _ in 0..5 {
        let output = idle(&mut served.region);
        assert!(!output.claims.contains(&ACROSS));
        assert!(!output.chunk_requests.contains(&ACROSS));
    }
    assert!(served.region.chunk(ACROSS).is_none());
    // The lend's own call-off still takes it up.
    let output = served.tick(&TickInputs {
        lends_called_off: vec![(1, vec![ACROSS])],
        ..TickInputs::default()
    });
    serve_until_loaded(&mut served, ACROSS, output);
}

/// Section 4: the borrower keeps a lent chunk loaded without tickets, and the rule that
/// gives back a chunk unused (here at once) does not give it back.
#[test]
fn a_lent_chunk_stays_loaded_without_tickets_and_is_not_given_back_unused() {
    let mut served = borrower_with_the_lend();
    for _ in 0..40 {
        let output = served.tick(&TickInputs::default());
        assert!(!output.returns.contains(&ACROSS), "tick {}", output.tick);
        assert!(
            served.region.chunk(ACROSS).is_some(),
            "tick {}",
            output.tick
        );
    }
    assert_eq!(served.region.knowledge(ACROSS), Knowledge::Held);
}

/// Section 4: once the last part near it is dug, the lent chunk is given back, loaded at
/// the end of that tick and dropped in the next.
#[test]
fn a_lent_chunk_is_given_back_when_the_part_near_it_is_dug() {
    let mut served = borrower_with_the_lend();
    let output = served.tick(&digging(1, -60, 2));
    assert_eq!(
        block_at(&served.region, BlockPos::new(1, -60, 2)),
        Some(blocks::AIR)
    );
    assert_eq!(output.returns, [ACROSS]);
    assert!(served.region.chunk(ACROSS).is_some(), "loaded to be saved");
    let output = served.tick(&TickInputs::default());
    assert!(served.region.chunk(ACROSS).is_none());
    assert!(!output.returns.contains(&ACROSS));
}

/// Section 4: the lent chunk is not given back while a part of the region's own land
/// beside still reaches it.
#[test]
fn a_lent_chunk_is_kept_while_another_part_beside_reaches_it() {
    let mut served = borrower_with_the_lend();
    let output = place_part(&mut served.region, items::REDSTONE_TORCH, 0, 6);
    assert!(
        output.lends.is_empty(),
        "it holds the chunk: {:?}",
        output.lends
    );
    let output = served.tick(&digging(1, -60, 2));
    assert!(output.returns.is_empty(), "{:?}", output.returns);
    for _ in 0..5 {
        assert!(served.tick(&TickInputs::default()).returns.is_empty());
    }
    let output = served.tick(&digging(0, -60, 6));
    assert_eq!(output.returns, [ACROSS]);
}

/// Section 4 with the crate's rule for returns (`TickOutput::returns` and `claims`): a
/// chunk given back is not claimed in the same tick, and the region knows nothing of it.
/// A borrower whose player looks across the line has a viewer's ticket on the lent chunk;
/// a claim that reached the store before the return was durable would find the chunk
/// still the borrower's and be granted, while the borrower drops it.
#[test]
fn a_lent_chunk_given_back_under_a_viewers_ticket_is_not_claimed_in_the_same_tick() {
    let (mut served, _) = borrower_asking();
    lend_to(&mut served, ACROSS, &[]);
    // The viewer's ticket the borrower had on the chunk across from the start stays.
    let output = served.region.tick(&digging(1, -60, 2));
    assert_eq!(output.returns, [ACROSS]);
    assert!(!output.claims.contains(&ACROSS), "{:?}", output.claims);
    assert_eq!(served.region.knowledge(ACROSS), Knowledge::Unknown);
}

/// Section 4: a part in the lent chunk keeps it.
#[test]
fn a_lent_chunk_is_kept_while_a_part_lies_in_it() {
    let mut served = borrower_with_the_lend();
    let output = place_part(&mut served.region, items::REDSTONE_TORCH, -1, 4);
    assert!(output.lends.is_empty());
    let output = served.tick(&digging(1, -60, 2));
    assert!(output.returns.is_empty(), "{:?}", output.returns);
    let output = served.tick(&digging(-1, -60, 4));
    assert_eq!(output.returns, [ACROSS]);
}

/// Section 4: the check is made too when a part near the lent chunk goes by an update: wire
/// whose floor is dug from under it.
#[test]
fn a_lent_chunk_is_given_back_when_the_part_near_it_goes_by_an_update() {
    let mut served = borrower_with_the_lend();
    let returned = served.tick(&digging(1, -61, 2)).returns;
    assert_eq!(
        block_at(&served.region, BlockPos::new(1, -60, 2)),
        Some(blocks::AIR)
    );
    let returned = if returned.is_empty() {
        served.tick(&TickInputs::default()).returns
    } else {
        returned
    };
    assert_eq!(returned, [ACROSS]);
}

/// A borrower restored holding `ACROSS` by a lend, with its own chunks near the line
/// loaded with `blocks`, and the ticks until they are loaded.
fn restored_borrower(blocks: &[(BlockPos, BlockState)]) -> (Region, Vec<TickOutput>) {
    let mut held = borrowers_own();
    held.push(ACROSS);
    let holdings = Holdings {
        held,
        pinned: vec![MIDDLE],
        lent: vec![ACROSS],
        lent_away: Vec::new(),
    };
    let state = knowing_the_edges(EntityIds::block(0).unwrap());
    let mut region = Region::restore(config(), state, holdings);
    let output = checked(&mut region, &tickets(borrowers_own(), vec![]));
    assert!(
        output.chunk_requests.contains(&ACROSS),
        "a lent chunk is kept loaded: {:?}",
        output.chunk_requests
    );
    let mut all = borrowers_own();
    all.push(ACROSS);
    let loaded = all
        .iter()
        .map(|position| (*position, floor_at(*position, blocks)))
        .collect();
    let output = checked(
        &mut region,
        &TickInputs {
            chunks_loaded: loaded,
            ..TickInputs::default()
        },
    );
    (region, vec![output])
}

/// Section 4: after a restore with `Holdings::lent` the lent chunk is kept, and looked at
/// once it is loaded: with no part near it, it is given back.
#[test]
fn after_a_restore_a_lent_chunk_with_no_part_near_is_looked_at_and_given_back() {
    let (mut region, outputs) = restored_borrower(&[]);
    let mut returned = outputs[0].returns.clone();
    for _ in 0..3 {
        if !returned.is_empty() {
            break;
        }
        returned = idle(&mut region).returns;
    }
    assert_eq!(returned, [ACROSS]);
}

/// Section 4: after a restore with `Holdings::lent` the lent chunk with a part near it is
/// kept, and not given back by the unused rule.
#[test]
fn after_a_restore_a_lent_chunk_with_a_part_near_is_kept() {
    for blocks in [
        [(BlockPos::new(1, -60, 2), blocks::REDSTONE_WIRE)],
        [(BlockPos::new(-3, -60, 2), blocks::REDSTONE_WIRE)],
    ] {
        let (mut region, outputs) = restored_borrower(&blocks);
        assert!(outputs[0].returns.is_empty(), "{blocks:?}");
        for _ in 0..40 {
            let output = idle(&mut region);
            assert!(
                output.returns.is_empty(),
                "{blocks:?}: {:?}",
                output.returns
            );
        }
        assert!(region.chunk(ACROSS).is_some());
        assert_eq!(region.knowledge(ACROSS), Knowledge::Held);
    }
}

/// Determinism: the state a run with lends starts from, with every tick's delta applied,
/// is the state of the region that ran (`RegionState::apply`), asks for lends among it.
#[test]
fn a_run_with_lends_is_its_first_state_and_its_deltas() {
    let mut served = borrower();
    let first = served.region.state();
    let mut deltas = Vec::new();
    let mut run = |served: &mut Served, inputs: TickInputs| {
        let output = served.tick(&inputs);
        deltas.push(output.delta.clone());
        output
    };
    let output = run(&mut served, placing(1, items::REDSTONE, 1, 2));
    assert_eq!(output.lends, [ACROSS]);
    run(
        &mut served,
        TickInputs {
            not_lent: vec![(vec![ACROSS], true)],
            ..TickInputs::default()
        },
    );
    let mut asked_again = false;
    for _ in 0..25 {
        asked_again |= !run(&mut served, TickInputs::default()).lends.is_empty();
    }
    assert!(asked_again);
    served.grants.granted.insert(ACROSS, BORROWER);
    run(&mut served, answers(vec![ACROSS], vec![]));
    run(&mut served, loaded_floor(ACROSS));
    run(&mut served, tickets(vec![], vec![ACROSS]));
    let output = run(&mut served, digging(1, -60, 2));
    assert_eq!(output.returns, [ACROSS]);
    run(&mut served, TickInputs::default());

    let mut replayed = first;
    for delta in &deltas {
        replayed.apply(delta);
    }
    assert_eq!(replayed, served.region.state());
}
