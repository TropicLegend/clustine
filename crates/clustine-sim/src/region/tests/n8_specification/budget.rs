//! The budget of chunks a light step takes (the plan's 4.5 and section 14, ADR-0029): 8
//! units a chunk lit whole and 1 a chunk adopted, nearest the region's players first
//! and then by position, a chunk at the head that does not fit holding back the rest; a
//! chunk ticking only once its 3×3 is lit; a replacement lit outside the budget; a
//! region made by `restore_loaded` within the budget going on as the original.

use super::*;

/// The 5×5 of chunks about the origin.
fn square() -> Vec<ChunkPos> {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// Which of the 5×5 are lit.
fn lit(region: &Region) -> Vec<ChunkPos> {
    square()
        .into_iter()
        .filter(|chunk| {
            region
                .light_at(BlockPos::new(chunk.x * 16 + 3, GROUND, chunk.z * 16 + 5))
                .is_some()
        })
        .collect()
}

/// `region` given viewers' tickets on the 5×5 and then its chunks, the floor with
/// `kept` light for some, within `budget`: the tick they come in.
fn load_square(region: &mut Region, budget: Option<u32>, kept: Vec<(ChunkPos, crate::KeptLight)>) {
    region.config.light.budget = budget;
    region.tick(&tickets(square(), vec![]));
    region.tick(&TickInputs {
        chunks_loaded: square().into_iter().map(|at| (at, floor())).collect(),
        chunks_kept: kept,
        ..TickInputs::default()
    });
}

fn chunks(list: &[(i32, i32)]) -> Vec<ChunkPos> {
    let mut chunks: Vec<ChunkPos> = list.iter().map(|(x, z)| ChunkPos::new(*x, *z)).collect();
    chunks.sort();
    chunks
}

#[test]
fn a_step_lights_the_player_s_chunk_and_then_the_nearest_by_position_three_at_a_time() {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    // 24 units: three chunks lit whole a step.
    load_square(&mut region, Some(24), Vec::new());
    assert_eq!(lit(&region), chunks(&[(0, 0), (-1, -1), (-1, 0)]));
    assert!(!region.ticks_chunk(ChunkPos::new(0, 0)));
    idle(&mut region);
    assert_eq!(
        lit(&region),
        chunks(&[(0, 0), (-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1)])
    );
    assert!(!region.ticks_chunk(ChunkPos::new(0, 0)), "six of its nine");
    idle(&mut region);
    // The 3×3 about the player is lit: its centre ticks from this very tick.
    let ring: Vec<(i32, i32)> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| (x, z)))
        .collect();
    assert_eq!(lit(&region), chunks(&ring));
    assert!(region.ticks_chunk(ChunkPos::new(0, 0)));
    assert!(
        !region.ticks_chunk(ChunkPos::new(1, 0)),
        "its neighbours wait"
    );
    idle(&mut region);
    // Then the far ring, from the west.
    let mut next = ring.clone();
    next.extend([(-2, -2), (-2, -1), (-2, 0)]);
    assert_eq!(lit(&region), chunks(&next));
}

#[test]
fn a_region_without_players_lights_its_chunks_by_position_alone() {
    let mut region = region();
    assert!(region.players.is_empty());
    load_square(&mut region, Some(16), Vec::new());
    assert_eq!(lit(&region), chunks(&[(-2, -2), (-2, -1)]));
    idle(&mut region);
    assert_eq!(
        lit(&region),
        chunks(&[(-2, -2), (-2, -1), (-2, 0), (-2, 1)])
    );
}

/// Light handed out for the 5×5 by a region that lit them all.
fn kept_square() -> Vec<(ChunkPos, crate::KeptLight)> {
    let mut donor = joined_in(ChunkArea::EVERYWHERE, &[1]);
    load_square(&mut donor, None, Vec::new());
    donor.hand_over_light()
}

#[test]
fn a_chunk_at_the_head_that_does_not_fit_holds_back_a_cheaper_one_behind_it() {
    // The chunk at (-1, 0) comes with kept light, at one unit; the rest are lit whole.
    let kept: Vec<(ChunkPos, crate::KeptLight)> = kept_square()
        .into_iter()
        .filter(|(at, _)| *at == ChunkPos::new(-1, 0))
        .collect();
    assert_eq!(kept.len(), 1);
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    // 12 units: the player's chunk (8), then (-1, -1) at 8 does not fit in the 4 left,
    // and (-1, 0) behind it, at 1, waits too.
    load_square(&mut region, Some(12), kept);
    assert_eq!(lit(&region), chunks(&[(0, 0)]));
    idle(&mut region);
    // The next step: (-1, -1) whole, (-1, 0) adopted at 1, and 3 left for nothing whole.
    assert_eq!(lit(&region), chunks(&[(0, 0), (-1, -1), (-1, 0)]));
}

#[test]
fn chunks_that_come_with_kept_light_are_taken_at_one_unit_each_and_give_the_same_light() {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    // 25 units take all 25 adopted in one step, where lit whole they would take 25 steps.
    load_square(&mut region, Some(25), kept_square());
    assert_eq!(lit(&region).len(), 25);
    assert!(region.ticks_chunk(ChunkPos::new(0, 0)));
    let mut whole = joined_in(ChunkArea::EVERYWHERE, &[1]);
    load_square(&mut whole, None, Vec::new());
    for x in -40..40 {
        for y in [-62, GROUND, 0] {
            let pos = at(x, y, x / 2);
            assert_eq!(levels(&region, pos), levels(&whole, pos), "{pos:?}");
        }
    }
}

#[test]
fn nothing_is_taken_with_a_budget_of_nothing_and_no_chunk_ticks() {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    load_square(&mut region, Some(0), Vec::new());
    for _ in 0..3 {
        idle(&mut region);
    }
    assert!(lit(&region).is_empty());
    assert!(region.ticking_chunks().is_empty());
    // Reads there say not judged.
    assert_eq!(region.raw_brightness(at(0, GROUND, 0), 0), None);
    assert_eq!(region.block_brightness(at(0, GROUND, 0)), None);
}

/// A region holding the chunks west of x = 32, ticking the 3×3 about the origin, with
/// the column of chunks at x = 2 another region's and copied; all lit.
fn at_a_line() -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(2),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    region.tick(&TickInputs {
        chunks_loaded: around.iter().map(|at| (*at, floor())).collect(),
        ..TickInputs::default()
    });
    let copies: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(2, z)).collect();
    region.tick(&TickInputs {
        foreign: copies.iter().map(|at| (*at, RegionId(1))).collect(),
        copy_bases: copies.iter().map(|at| (*at, floor())).collect(),
        ..TickInputs::default()
    });
    idle(&mut region);
    region
}

#[test]
fn a_copy_replaced_whole_is_lit_in_its_own_step_and_the_chunk_beside_it_ticks_on() {
    let mut region = at_a_line();
    let beside = ChunkPos::new(1, 0);
    assert!(region.ticks_chunk(beside));
    assert!(region.copies.contains_key(&ChunkPos::new(2, 0)));
    assert_eq!(levels(&region, at(31, GROUND, 8)), Some((15, 0)));
    // No budget left for anything new; the new base holds a torch by the line.
    region.config.light.budget = Some(0);
    let mut based = floor();
    based.set(0, GROUND, 8, blocks::TORCH);
    region.tick(&TickInputs {
        copy_bases: vec![(ChunkPos::new(2, 0), based)],
        ..TickInputs::default()
    });
    assert!(
        region.ticks_chunk(beside),
        "it does not stop for the new base"
    );
    assert_eq!(levels(&region, at(32, GROUND, 8)), Some((15, 14)));
    assert_eq!(levels(&region, at(31, GROUND, 8)), Some((15, 13)));
    assert_eq!(levels(&region, at(28, GROUND, 8)), Some((15, 10)));
    // And a base without it takes its light away again, in its step.
    region.tick(&TickInputs {
        copy_bases: vec![(ChunkPos::new(2, 0), floor())],
        ..TickInputs::default()
    });
    assert!(region.ticks_chunk(beside));
    assert_eq!(levels(&region, at(31, GROUND, 8)), Some((15, 0)));
}

/// The state and saved chunks of `region`'s 5×5 as a restore takes them, restored with
/// `budget`.
fn restored(region: &Region, budget: Option<u32>) -> Region {
    let chunks: Vec<(ChunkPos, Chunk)> = square()
        .into_iter()
        .map(|at| (at, region.saved_chunk(at).expect("loaded")))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = square()
        .into_iter()
        .map(|at| (at, Ticket::Viewer))
        .collect();
    let mut config = region.config.clone();
    config.light.budget = budget;
    Region::restore_loaded(
        config,
        region.state(),
        Holdings {
            pinned: Vec::new(),
            held: square(),
            lent: Vec::new(),
            lent_away: Vec::new(),
        },
        &tickets,
        chunks,
        Vec::new(),
    )
}

/// A field of wheat at noon, its farmland moist, ticking in the 5×5 about the origin
/// with a torch in a covered pit.
fn wheat_field() -> Region {
    let mut placed = Vec::new();
    for x in 2..12 {
        for z in 2..12 {
            placed.push((
                at(x, GROUND - 1, z),
                state("minecraft:farmland[moisture=7]"),
            ));
            placed.push((at(x, GROUND, z), blocks::WHEAT));
        }
    }
    placed.push((at(20, GROUND, 20), blocks::GLOWSTONE));
    let mut region = super::super::redstone::ticking_with(&placed);
    region.clock.day = NOON;
    region
}

fn ages(region: &Region) -> Vec<BlockState> {
    (2..12)
        .flat_map(|x| (2..12).map(move |z| at(x, GROUND, z)))
        .map(|pos| read(region, pos))
        .collect()
}

#[test]
fn a_region_restored_in_the_middle_of_a_growth_within_the_budget_goes_on_as_the_original() {
    let mut original = wheat_field();
    // The server's budget, which 25 chunks lit whole (200 units) fit in.
    original.config.light.budget = Some(256);
    for _ in 0..300 {
        idle(&mut original);
    }
    let grown = ages(&original);
    assert!(grown.iter().any(|age| *age != blocks::WHEAT), "some grew");
    let mut restored = restored(&original, Some(256));
    for _ in 0..400 {
        let a = idle(&mut original);
        let b = idle(&mut restored);
        assert_eq!(a.tick, b.tick);
        assert_eq!(a.events, b.events, "tick {}", a.tick);
    }
    for at in square() {
        assert_eq!(restored.saved_chunk(at), original.saved_chunk(at), "{at:?}");
    }
    assert_eq!(ages(&restored), ages(&original));
    assert_ne!(ages(&restored), grown, "it grew on");
    for pos in [at(20, GROUND, 21), at(5, GROUND + 1, 5), at(-30, -62, 30)] {
        assert_eq!(levels(&restored, pos), levels(&original, pos), "{pos:?}");
    }
}

#[test]
fn a_region_restored_beyond_its_budget_waits_for_its_light_and_ticks_nothing_unlit() {
    let mut original = wheat_field();
    for _ in 0..5 {
        idle(&mut original);
    }
    // 64 units: eight chunks lit whole a step, nearest the player first.
    let mut restored = restored(&original, Some(64));
    let output = idle(&mut restored);
    assert_eq!(lit(&restored).len(), 8);
    assert!(restored.ticking_chunks().is_empty(), "{:?}", output.tick);
    idle(&mut restored);
    assert_eq!(lit(&restored).len(), 16);
    assert!(restored.ticks_chunk(ChunkPos::new(0, 0)));
    for _ in 0..2 {
        idle(&mut restored);
    }
    assert_eq!(lit(&restored).len(), 25);
    assert!(restored.ticks_chunk(ChunkPos::new(1, 1)));
}
