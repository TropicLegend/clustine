//! Pistons (sections 3, 5 and 9 of the record), against the jar's `PistonBaseBlock`,
//! `PistonStructureResolver` and `PistonMovingBlockEntity`: the block events, the moving
//! piston's three ticks, the line of twelve, slime and honey, sticky pistons pulling and
//! dropping their block on a short pulse, quasi-connectivity, a piston powered through
//! the block above it, a piston door, an observer watching a piston and one powering a
//! sticky piston, whose retraction is instant because the level handles its tick.

use clustine_data::sounds as s;

use super::*;
use crate::api::Face;

const PISTON_AT: BlockPos = BlockPos::new(3, -60, 3);
const HEAD: BlockPos = BlockPos::new(4, -60, 3);
const AHEAD: BlockPos = BlockPos::new(5, -60, 3);
const LEVER: BlockPos = BlockPos::new(3, -60, 2);

/// East, as `Direction.get3DDataValue` numbers it, which a piston's block events carry.
const EAST: i32 = 5;

fn piston(extended: bool) -> BlockState {
    state(&format!(
        "minecraft:piston[extended={extended},facing=east]"
    ))
}

fn sticky(extended: bool) -> BlockState {
    state(&format!(
        "minecraft:sticky_piston[extended={extended},facing=east]"
    ))
}

fn is_moving(region: &Region, pos: BlockPos) -> bool {
    block_at(region, pos).and_then(BlockState::block_id) == blocks::MOVING_PISTON.block_id()
}

fn head(kind: &str) -> BlockState {
    state(&format!(
        "minecraft:piston_head[facing=east,short=false,type={kind}]"
    ))
}

/// A piston powered by a lever queues block event 0 with its direction; the event, in
/// the same tick's phase of block events, sets the base extended, the head and the
/// pushed block moving, and plays the extension, before the block event is sent. The
/// moving pistons are ticked in that tick, the next, and finish in the one after
/// (section 5).
#[test]
fn a_piston_pushes_a_block_and_the_move_finishes_two_ticks_after_its_block_event() {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON_AT, piston(false)),
        (HEAD, blocks::STONE),
    ]);
    let output = step(&mut region, &click(LEVER));
    assert_eq!(
        block_events(&output),
        [(PISTON_AT, block_id(piston(false)), 0, EAST)]
    );
    let extend = output
        .events
        .iter()
        .position(|event| matches!(event, RegionEvent::Sound { sound, .. } if *sound == s::BLOCK_PISTON_EXTEND))
        .expect("the extension is heard");
    let sent = output
        .events
        .iter()
        .position(|event| matches!(event, RegionEvent::BlockEvent { .. }))
        .expect("the event is sent");
    assert!(extend < sent);
    assert_eq!(block_at(&region, PISTON_AT), Some(piston(true)));
    assert!(is_moving(&region, HEAD) && is_moving(&region, AHEAD));
    // The moving pistons are block entities of their chunk, which keeps their data.
    assert!(block_entity_data(&region, HEAD).is_some_and(|data| !data.is_empty()));
    assert!(block_entity_data(&region, AHEAD).is_some_and(|data| !data.is_empty()));
    idle(&mut region);
    assert!(is_moving(&region, HEAD) && is_moving(&region, AHEAD));
    idle(&mut region);
    assert_eq!(block_at(&region, HEAD), Some(head("normal")));
    assert_eq!(block_at(&region, AHEAD), Some(blocks::STONE));
    assert_eq!(block_entity_data(&region, AHEAD), None);
}

/// Unpowered, an extended piston queues event 1: its base becomes a moving piston at
/// once and its head goes, and the base is back two ticks later. The block it pushed
/// stays.
#[test]
fn a_piston_retracts_its_head_at_once_and_its_base_returns_two_ticks_later() {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON_AT, piston(false)),
        (HEAD, blocks::STONE),
    ]);
    step(&mut region, &click(LEVER));
    for _ in 0..3 {
        idle(&mut region);
    }
    let output = step(&mut region, &click(LEVER));
    assert_eq!(
        block_events(&output),
        [(PISTON_AT, block_id(piston(false)), 1, EAST)]
    );
    assert!(sound_ids(&output).contains(&s::BLOCK_PISTON_CONTRACT));
    assert!(is_moving(&region, PISTON_AT));
    assert_eq!(block_at(&region, HEAD), Some(blocks::AIR));
    idle(&mut region);
    assert!(is_moving(&region, PISTON_AT));
    idle(&mut region);
    assert_eq!(block_at(&region, PISTON_AT), Some(piston(false)));
    assert_eq!(block_at(&region, AHEAD), Some(blocks::STONE));
}

/// `PistonStructureResolver`: a piston moves at most twelve blocks. Thirteen in a line
/// and it queues no event at all.
#[test]
fn a_piston_pushes_a_line_of_twelve_blocks_and_not_thirteen() {
    for length in [12, 13] {
        let line: Vec<BlockPos> = (0..length).map(|i| at(4 + i, -60, 3)).collect();
        let mut blocks = vec![(LEVER, lever(false)), (PISTON_AT, piston(false))];
        blocks.extend(line.iter().map(|pos| (*pos, blocks::STONE)));
        let mut region = ticking_with(&blocks);
        let output = step(&mut region, &click(LEVER));
        idle(&mut region);
        idle(&mut region);
        if length == 12 {
            assert_eq!(block_events(&output).len(), 1);
            assert_eq!(block_at(&region, HEAD), Some(head("normal")));
            for i in 0..12 {
                assert_eq!(block_at(&region, at(5 + i, -60, 3)), Some(blocks::STONE));
            }
        } else {
            assert_eq!(block_events(&output), []);
            assert!(!sound_ids(&output).contains(&s::BLOCK_PISTON_EXTEND));
            assert_eq!(block_at(&region, PISTON_AT), Some(piston(false)));
            for pos in &line {
                assert_eq!(block_at(&region, *pos), Some(blocks::STONE));
            }
            assert_eq!(block_at(&region, at(17, -60, 3)), Some(blocks::AIR));
        }
    }
}

/// Obsidian, and any block with a block entity, cannot be pushed; a block that breaks
/// when pushed (a flower) is destroyed and the line moves into its place.
#[test]
fn a_piston_does_not_push_obsidian_or_a_chest_and_breaks_a_flower() {
    for immovable in [
        blocks::OBSIDIAN,
        state("minecraft:chest[facing=north,type=single,waterlogged=false]"),
    ] {
        let mut region = ticking_with(&[
            (LEVER, lever(false)),
            (PISTON_AT, piston(false)),
            (HEAD, immovable),
        ]);
        let output = step(&mut region, &click(LEVER));
        assert_eq!(block_events(&output), [], "{immovable:?}");
        assert_eq!(block_at(&region, PISTON_AT), Some(piston(false)));
        assert_eq!(block_at(&region, HEAD), Some(immovable));
    }
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON_AT, piston(false)),
        (HEAD, blocks::STONE),
        (AHEAD, blocks::DANDELION),
    ]);
    step(&mut region, &click(LEVER));
    idle(&mut region);
    idle(&mut region);
    assert_eq!(block_at(&region, AHEAD), Some(blocks::STONE));
    assert_eq!(block_at(&region, at(6, -60, 3)), Some(blocks::AIR));
}

/// The slime contraptions are built in the air, where the floor would stick to slime.
const HIGH: i32 = -58;

/// A redstone block set against the north face of the piston at `at`, and dug again.
fn power_by_hand(region: &mut Region, piston_at: BlockPos) {
    step(region, &holding(items::REDSTONE_BLOCK));
    step(region, &use_on(piston_at, Face::North));
    let placed = piston_at.offset(0, 0, -1);
    assert_eq!(block_at(region, placed), Some(blocks::REDSTONE_BLOCK));
}

fn unpower_by_hand(region: &mut Region, piston_at: BlockPos) {
    step(region, &dig_at(piston_at.offset(0, 0, -1)));
}

/// Slime moves the blocks beside it that can be moved, and a sticky piston pulls the
/// whole of it back.
#[test]
fn slime_takes_its_neighbours_along_both_ways() {
    let base = at(3, HIGH, 3);
    let slime = at(4, HIGH, 3);
    let beside = at(4, HIGH, 4);
    let above = at(4, HIGH + 1, 3);
    let mut region = ticking_with(&[
        (base, sticky(false)),
        (slime, blocks::SLIME_BLOCK),
        (beside, blocks::STONE),
        (above, blocks::STONE),
    ]);
    power_by_hand(&mut region, base);
    idle(&mut region);
    idle(&mut region);
    let east = |pos: BlockPos| pos.offset(1, 0, 0);
    assert_eq!(block_at(&region, east(slime)), Some(blocks::SLIME_BLOCK));
    assert_eq!(block_at(&region, east(beside)), Some(blocks::STONE));
    assert_eq!(block_at(&region, east(above)), Some(blocks::STONE));
    assert_eq!(block_at(&region, beside), Some(blocks::AIR));
    assert_eq!(block_at(&region, above), Some(blocks::AIR));
    idle(&mut region);
    unpower_by_hand(&mut region, base);
    idle(&mut region);
    idle(&mut region);
    assert_eq!(block_at(&region, base), Some(sticky(false)));
    assert_eq!(block_at(&region, slime), Some(blocks::SLIME_BLOCK));
    assert_eq!(block_at(&region, beside), Some(blocks::STONE));
    assert_eq!(block_at(&region, above), Some(blocks::STONE));
    assert_eq!(block_at(&region, east(slime)), Some(blocks::AIR));
}

/// Honey and slime do not stick to each other (`canStickToEachOther`): honey takes the
/// stone on it and leaves the slime beside it.
#[test]
fn honey_leaves_slime_beside_it_behind() {
    let base = at(3, HIGH, 3);
    let honey = at(4, HIGH, 3);
    let slime = at(4, HIGH, 4);
    let above = at(4, HIGH + 1, 3);
    let mut region = ticking_with(&[
        (base, state("minecraft:piston[extended=false,facing=east]")),
        (honey, blocks::HONEY_BLOCK),
        (slime, blocks::SLIME_BLOCK),
        (above, blocks::STONE),
    ]);
    power_by_hand(&mut region, base);
    idle(&mut region);
    idle(&mut region);
    assert_eq!(
        block_at(&region, honey.offset(1, 0, 0)),
        Some(blocks::HONEY_BLOCK)
    );
    assert_eq!(
        block_at(&region, above.offset(1, 0, 0)),
        Some(blocks::STONE)
    );
    assert_eq!(block_at(&region, slime), Some(blocks::SLIME_BLOCK));
    assert_eq!(block_at(&region, slime.offset(1, 0, 0)), Some(blocks::AIR));
}

/// A sticky piston unpowered once its block has arrived pulls it back: the block moves
/// into the head's place, arriving two ticks after the event.
#[test]
fn a_sticky_piston_pulls_its_block_back() {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON_AT, sticky(false)),
        (HEAD, blocks::STONE),
    ]);
    step(&mut region, &click(LEVER));
    for _ in 0..4 {
        idle(&mut region);
    }
    assert_eq!(block_at(&region, AHEAD), Some(blocks::STONE));
    let output = step(&mut region, &click(LEVER));
    assert_eq!(
        block_events(&output),
        [(PISTON_AT, block_id(sticky(false)), 1, EAST)]
    );
    assert!(is_moving(&region, PISTON_AT) && is_moving(&region, HEAD));
    assert_eq!(block_at(&region, AHEAD), Some(blocks::AIR));
    idle(&mut region);
    idle(&mut region);
    assert_eq!(block_at(&region, PISTON_AT), Some(sticky(false)));
    assert_eq!(block_at(&region, HEAD), Some(blocks::STONE));
}

/// A sticky piston that is turned on by a player's lever and off again `pulse` ticks
/// later; returns the block event of its retraction and whether its block was dropped
/// where it was going rather than pulled back.
fn sticky_pulse(pulse: usize) -> (Vec<(BlockPos, u16, i32, i32)>, bool) {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON_AT, sticky(false)),
        (HEAD, blocks::STONE),
    ]);
    step(&mut region, &click(LEVER));
    for _ in 1..pulse {
        idle(&mut region);
    }
    let output = step(&mut region, &click(LEVER));
    for _ in 0..3 {
        idle(&mut region);
    }
    assert_eq!(block_at(&region, PISTON_AT), Some(sticky(false)), "{pulse}");
    let dropped = block_at(&region, AHEAD) == Some(blocks::STONE);
    let pulled = block_at(&region, HEAD) == Some(blocks::STONE);
    assert!(dropped != pulled, "a pulse of {pulse}");
    (block_events(&output), dropped)
}

/// A sticky piston unpowered while its block still moves drops it where it was going
/// (`triggerEvent`: the moving piston two ahead is finished in place and nothing is
/// pulled). Two ticks on, the move is half way and was ticked the tick before, and a
/// player's input does not handle the tick, so the retraction is not instant (event
/// 1), and the block, still moving, is dropped. Three ticks on, it has arrived and is
/// pulled.
#[test]
fn a_sticky_piston_on_a_short_pulse_drops_its_block() {
    let id = block_id(sticky(false));
    assert_eq!(sticky_pulse(2), (vec![(PISTON_AT, id, 1, EAST)], true));
    assert_eq!(sticky_pulse(3), (vec![(PISTON_AT, id, 1, EAST)], false));
}

/// One tick on, the jar's `checkIfExtend` asks `getProgress(0.0)`, which is
/// `progressO`: the progress before the moving piston's last tick, 0 after its first.
/// That is less than half way, so the retraction is instant, event 2 (section 9 of the
/// record: "less than half way").
#[test]
fn a_sticky_piston_unpowered_one_tick_on_retracts_at_once() {
    let id = block_id(sticky(false));
    assert_eq!(sticky_pulse(1), (vec![(PISTON_AT, id, 2, EAST)], true));
}

/// Quasi-connectivity: a piston is powered by what would power the block above it,
/// here a lever two above with air between. The lever's change reaches no neighbour of
/// the piston, so the piston waits, powered, until a neighbour changes.
#[test]
fn a_piston_powered_from_two_above_extends_only_when_a_neighbour_changes() {
    let wall_lever = at(3, -58, 3);
    let mut region = ticking_with(&[
        (PISTON_AT, piston(false)),
        (at(3, -58, 2), blocks::STONE),
        (
            wall_lever,
            state("minecraft:lever[face=wall,facing=south,powered=false]"),
        ),
    ]);
    step(&mut region, &click(wall_lever));
    assert!(on(&region, wall_lever, "powered"));
    let extended = watch(&mut region, 3, |region| block_at(region, PISTON_AT));
    assert_eq!(extended, [Some(piston(false)); 3]);
    let output = step(&mut region, &use_on(at(3, -61, 4), Face::Top));
    assert_eq!(block_at(&region, at(3, -60, 4)), Some(blocks::STONE));
    assert_eq!(block_events(&output).len(), 1);
    assert_eq!(block_at(&region, PISTON_AT), Some(piston(true)));
}

/// A block above the piston strongly powered by a lever on it powers the piston, and
/// the lever's change reaches the piston through the neighbours of the block it is on.
#[test]
fn a_piston_is_powered_through_the_block_above_it() {
    let top = at(3, -58, 3);
    let mut region = ticking_with(&[
        (PISTON_AT, piston(false)),
        (at(3, -59, 3), blocks::STONE),
        (top, lever(false)),
    ]);
    let output = step(&mut region, &click(top));
    assert_eq!(block_events(&output).len(), 1);
    assert_eq!(block_at(&region, PISTON_AT), Some(piston(true)));
}

/// A door of two sticky pistons, one above the other, by a redstone block set beside
/// the upper one: the lower one is powered only by quasi-connectivity and told only
/// when the upper one's base is set extended, so its event is queued while the phase
/// runs and runs in the same phase, after the upper one's. Taken away, both pull back
/// in one tick the same way round.
#[test]
fn a_piston_door_opens_and_closes_in_one_tick_the_upper_piston_first() {
    let lower = at(2, -60, 3);
    let upper = at(2, -59, 3);
    let support = at(1, -60, 3);
    let mut region = ticking_with(&[
        (lower, sticky(false)),
        (upper, sticky(false)),
        (at(3, -60, 3), blocks::STONE),
        (at(3, -59, 3), blocks::STONE),
        (support, blocks::STONE),
    ]);
    step(&mut region, &holding(items::REDSTONE_BLOCK));
    let output = step(&mut region, &use_on(support, Face::Top));
    assert_eq!(
        block_at(&region, at(1, -59, 3)),
        Some(blocks::REDSTONE_BLOCK)
    );
    let id = block_id(sticky(false));
    assert_eq!(
        block_events(&output),
        [(upper, id, 0, EAST), (lower, id, 0, EAST)]
    );
    idle(&mut region);
    idle(&mut region);
    assert_eq!(block_at(&region, at(4, -60, 3)), Some(blocks::STONE));
    assert_eq!(block_at(&region, at(4, -59, 3)), Some(blocks::STONE));
    idle(&mut region);
    let output = step(&mut region, &dig_at(at(1, -59, 3)));
    assert_eq!(
        block_events(&output),
        [(upper, id, 1, EAST), (lower, id, 1, EAST)]
    );
    idle(&mut region);
    idle(&mut region);
    assert_eq!(block_at(&region, lower), Some(sticky(false)));
    assert_eq!(block_at(&region, upper), Some(sticky(false)));
    assert_eq!(block_at(&region, at(3, -60, 3)), Some(blocks::STONE));
    assert_eq!(block_at(&region, at(3, -59, 3)), Some(blocks::STONE));
}

/// An observer watching a piston sees its base set extended in the phase of block
/// events, and pulses two ticks later.
#[test]
fn an_observer_watching_a_piston_pulses_two_ticks_after_it_extends() {
    let observer = at(2, -60, 3);
    let lamp = at(1, -60, 3);
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON_AT, piston(false)),
        (
            observer,
            state("minecraft:observer[facing=east,powered=false]"),
        ),
        (lamp, blocks::REDSTONE_LAMP),
    ]);
    step(&mut region, &click(LEVER));
    assert_eq!(block_at(&region, PISTON_AT), Some(piston(true)));
    let lit = watch(&mut region, 8, |region| on(region, lamp, "lit"));
    assert_eq!(lit, [false, true, true, true, true, true, true, false]);
}

/// An observer's pulse into a sticky piston: the observer turns off in the phase of
/// block ticks, which handles the tick (section 3), while the block is still moving and
/// past half way, so the retraction is instant (event 2) and the block is dropped.
#[test]
fn an_observers_pulse_into_a_sticky_piston_retracts_it_at_once_and_drops_its_block() {
    let observer = at(2, -60, 3);
    let mut region = ticking_with(&[
        (
            observer,
            state("minecraft:observer[facing=west,powered=false]"),
        ),
        (PISTON_AT, sticky(false)),
        (HEAD, blocks::STONE),
    ]);
    step(&mut region, &use_on(at(1, -61, 3), Face::Top));
    assert_eq!(block_at(&region, at(1, -60, 3)), Some(blocks::STONE));
    let id = block_id(sticky(false));
    let events: Vec<_> = (1..=6).map(|_| block_events(&idle(&mut region))).collect();
    assert_eq!(
        events,
        [
            vec![],
            vec![(PISTON_AT, id, 0, EAST)],
            vec![],
            vec![(PISTON_AT, id, 2, EAST)],
            vec![],
            vec![]
        ]
    );
    assert_eq!(block_at(&region, PISTON_AT), Some(sticky(false)));
    assert_eq!(block_at(&region, HEAD), Some(blocks::AIR));
    assert_eq!(block_at(&region, AHEAD), Some(blocks::STONE));
}

/// The game saves a moving piston's progress before its last tick and loads both as
/// that, so a move carried through a restore half way takes a tick more (section 5).
#[test]
fn a_move_carried_through_a_restore_half_way_takes_a_tick_more() {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON_AT, piston(false)),
        (HEAD, blocks::STONE),
    ]);
    step(&mut region, &click(LEVER));
    idle(&mut region);
    let mut restored = super::super::scheduled_ticks::restored(&region);
    assert!(is_moving(&restored, AHEAD));
    idle(&mut region);
    assert_eq!(block_at(&region, AHEAD), Some(blocks::STONE));
    idle(&mut restored);
    assert!(is_moving(&restored, AHEAD), "a tick more");
    idle(&mut restored);
    assert_eq!(block_at(&restored, AHEAD), Some(blocks::STONE));
    assert_eq!(block_at(&restored, HEAD), Some(head("normal")));
}
