//! Tests of pistons (step R5 of `docs/adr/0026-redstone.md`, section 9), from the
//! game's `PistonBaseBlock` and `PistonMovingBlockEntity`: a move in three ticks of the
//! block entities' phase, a sticky piston's pull, the line of twelve, a short pulse,
//! the block event and the sounds, a move carried through a save, and a piston that
//! waits for its land.
//!
//! A lever stands on the floor of chunk (0, 0) at (0, -60, 2), and a piston east of it
//! faces east.

use clustine_data::{BlockClass, sounds as s};

use super::redstone::{block_at, click, lever, sounds, state, ticking_with};
use super::*;

const LEVER: BlockPos = BlockPos::new(0, -60, 2);
const PISTON: BlockPos = BlockPos::new(1, -60, 2);

fn piston(kind: &str, extended: bool) -> BlockState {
    state(&format!(
        "minecraft:{kind}[facing=east,extended={extended}]"
    ))
}

fn east(n: i32) -> BlockPos {
    BlockPos::new(PISTON.x + n, PISTON.y, PISTON.z)
}

fn is(region: &Region, pos: BlockPos, class: BlockClass) -> bool {
    block_at(region, pos).is_some_and(|state| clustine_blocks::data::is_class(state, class))
}

#[test]
fn a_piston_pushes_a_block_one_on_over_three_ticks_and_ends_with_its_head_out() {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON, piston("piston", false)),
        (east(1), blocks::STONE),
    ]);
    let pushed = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, PISTON), Some(piston("piston", true)));
    assert!(
        is(&region, east(1), BlockClass::MovingPistonBlock),
        "the head on its way"
    );
    assert!(
        is(&region, east(2), BlockClass::MovingPistonBlock),
        "the stone on its way"
    );
    // Its block event goes to the players near: 0 to extend, east being 5.
    let event = pushed.events.iter().find_map(|event| match event {
        RegionEvent::BlockEvent {
            position,
            kind,
            data,
            ..
        } => Some((*position, *kind, *data)),
        _ => None,
    });
    assert_eq!(event, Some((PISTON, 0, 5)));
    assert!(
        sounds(&pushed)
            .iter()
            .any(|(sound, volume, pitch, except)| {
                *sound == s::BLOCK_PISTON_EXTEND
                    && *volume == 0.5
                    && (0.6..0.85).contains(pitch)
                    && except.is_none()
            }),
        "{:?}",
        sounds(&pushed)
    );
    idle(&mut region);
    assert!(
        is(&region, east(2), BlockClass::MovingPistonBlock),
        "half way"
    );
    idle(&mut region);
    assert_eq!(block_at(&region, east(2)), Some(blocks::STONE));
    assert!(is(&region, east(1), BlockClass::PistonHeadBlock));
    assert_eq!(region.tickers(), [], "the moves are done");
}

#[test]
fn a_normal_piston_retracts_and_leaves_its_block_and_a_sticky_one_pulls_it_back() {
    for (kind, stone_ends) in [("piston", east(2)), ("sticky_piston", east(1))] {
        let mut region = ticking_with(&[
            (LEVER, lever(false)),
            (PISTON, piston(kind, false)),
            (east(1), blocks::STONE),
        ]);
        region.tick(&click(LEVER));
        idle(&mut region);
        idle(&mut region);
        let retracted = region.tick(&click(LEVER));
        assert!(
            sounds(&retracted)
                .iter()
                .any(|(sound, ..)| *sound == s::BLOCK_PISTON_CONTRACT),
            "{kind}"
        );
        for _ in 0..3 {
            idle(&mut region);
        }
        assert_eq!(
            block_at(&region, PISTON),
            Some(piston(kind, false)),
            "{kind}"
        );
        assert_eq!(block_at(&region, stone_ends), Some(blocks::STONE), "{kind}");
        let other = if stone_ends == east(1) {
            east(2)
        } else {
            east(1)
        };
        assert_eq!(block_at(&region, other), Some(blocks::AIR), "{kind}");
    }
}

#[test]
fn a_piston_does_not_push_thirteen_blocks_and_pushes_twelve() {
    for (count, moves) in [(12, true), (13, false)] {
        let mut blocks: Vec<(BlockPos, BlockState)> =
            vec![(LEVER, lever(false)), (PISTON, piston("piston", false))];
        blocks.extend((1..=count).map(|n| (east(n), blocks::STONE)));
        let mut region = ticking_with(&blocks);
        region.tick(&click(LEVER));
        assert_eq!(
            block_at(&region, PISTON) == Some(piston("piston", true)),
            moves,
            "{count}"
        );
    }
}

/// A sticky piston powered for a tick lets go of its block half way: the block stays
/// where it was pushed to, and the piston comes back without it.
#[test]
fn a_sticky_piston_powered_for_one_tick_leaves_its_block_out() {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON, piston("sticky_piston", false)),
        (east(1), blocks::STONE),
    ]);
    region.tick(&click(LEVER));
    region.tick(&click(LEVER));
    for _ in 0..3 {
        idle(&mut region);
    }
    assert_eq!(
        block_at(&region, PISTON),
        Some(piston("sticky_piston", false))
    );
    assert_eq!(block_at(&region, east(2)), Some(blocks::STONE));
    assert_eq!(block_at(&region, east(1)), Some(blocks::AIR));
}

/// A move half way is in the chunk's data, as the game saves it, and a region restored
/// with the chunk finishes it, a tick later than it would have, as the game's does
/// after a reload.
#[test]
fn a_move_half_way_is_saved_with_its_chunk_and_finished_after_a_restore() {
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (PISTON, piston("piston", false)),
        (east(1), blocks::STONE),
    ]);
    let pushed = region.tick(&click(LEVER));
    assert!(
        pushed
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::BlockEntityChanged { position, .. } if *position == east(2))),
        "the moving block's data is said, to be committed"
    );
    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = around
        .iter()
        .map(|position| (*position, region.saved_chunk(*position).expect("loaded")))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = around
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let mut restored = Region::restore_loaded(
        region.config.clone(),
        region.state(),
        Holdings {
            pinned: Vec::new(),
            held: around.clone(),
            lent: Vec::new(),
            lent_away: Vec::new(),
        },
        &tickets,
        chunks,
        Vec::new(),
    );
    assert!(is(&restored, east(2), BlockClass::MovingPistonBlock));
    for _ in 0..2 {
        idle(&mut restored);
    }
    assert!(
        is(&restored, east(2), BlockClass::MovingPistonBlock),
        "a tick more"
    );
    idle(&mut restored);
    assert_eq!(block_at(&restored, east(2)), Some(blocks::STONE));
    assert!(is(&restored, east(1), BlockClass::PistonHeadBlock));
}

/// A head a player breaks takes its base with it, and a base broken takes its head.
#[test]
fn a_head_broken_takes_its_base_and_a_base_broken_takes_its_head() {
    for broken in [east(1), PISTON] {
        let mut region = ticking_with(&[(LEVER, lever(false)), (PISTON, piston("piston", false))]);
        region.tick(&click(LEVER));
        idle(&mut region);
        idle(&mut region);
        assert!(is(&region, east(1), BlockClass::PistonHeadBlock));
        region.tick(&moves(vec![dig(1, broken.x, broken.y, broken.z, 1)]));
        assert_eq!(block_at(&region, PISTON), Some(blocks::AIR), "{broken:?}");
        assert_eq!(block_at(&region, east(1)), Some(blocks::AIR), "{broken:?}");
    }
}

/// A piston whose push would reach a chunk the region has not loaded (one it reads as
/// a copy, as across a line) does not move, and waits as a seam entry, which stays put
/// tick after tick; once the chunk is loaded and ticks, the piston moves
/// (`docs/adr/0026-redstone.md`, section 10).
#[test]
fn a_piston_whose_push_reaches_a_chunk_not_loaded_waits_until_it_is() {
    let column = |x: i32| -> Vec<ChunkPos> { (-1..=1).map(|z| ChunkPos::new(x, z)).collect() };
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let loaded: Vec<ChunkPos> = column(-1).into_iter().chain(column(0)).collect();
    checked(&mut region, &tickets(loaded.clone(), vec![]));
    let edge_piston = BlockPos::new(15, -60, 2);
    let mut home = floor();
    home.set(15, -60, 2, piston("piston", false));
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: loaded
                .iter()
                .map(|position| {
                    let chunk = if *position == ChunkPos::new(0, 0) {
                        home.clone()
                    } else {
                        floor()
                    };
                    (*position, chunk)
                })
                .collect(),
            copy_bases: column(1)
                .into_iter()
                .map(|position| (position, floor()))
                .collect(),
            ..TickInputs::default()
        },
    );
    assert!(region.ticks_chunk(ChunkPos::new(0, 0)));
    assert!(!region.ticks_chunk(ChunkPos::new(1, 0)));
    // Power beside it.
    let mut output = TickOutput::default();
    region.set_block(
        BlockPos::new(15, -60, 3),
        blocks::REDSTONE_BLOCK,
        &mut output,
    );
    assert_eq!(
        block_at(&region, edge_piston),
        Some(piston("piston", false))
    );
    let waiting = |region: &Region| {
        region
            .seams(ChunkPos::new(0, 0))
            .iter()
            .filter(|seam| seam.at == edge_piston)
            .count()
    };
    assert_eq!(waiting(&region), 1);
    let quiet = idle(&mut region);
    assert_eq!(waiting(&region), 1);
    assert!(
        !quiet.seams_changed.contains(&ChunkPos::new(0, 0)),
        "kept as it was"
    );
    // Chunk 1 comes, with copies beyond it so that it ticks; the piston moves.
    checked(&mut region, &tickets(column(1), vec![]));
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: column(1)
                .into_iter()
                .map(|position| (position, floor()))
                .collect(),
            copy_bases: column(2)
                .into_iter()
                .map(|position| (position, floor()))
                .collect(),
            ..TickInputs::default()
        },
    );
    assert!(region.ticks_chunk(ChunkPos::new(1, 0)));
    assert_eq!(block_at(&region, edge_piston), Some(piston("piston", true)));
    assert_eq!(waiting(&region), 0);
}
