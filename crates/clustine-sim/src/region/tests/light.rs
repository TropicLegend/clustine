//! Light in the region (ADR-0029): the budget of what arrives, the rule that a chunk
//! ticks only once its neighbourhood is lit, and light handed on at a merge or a split.

use super::*;

fn idle_tick(region: &mut Region) {
    region.tick(&TickInputs::default());
}

/// A region of 25 chunks on the floor about the origin, lit within `budget`.
fn floor_of_25(budget: Option<u32>, kept: Vec<(ChunkPos, crate::KeptLight)>) -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    region.config.light.budget = budget;
    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    region.tick(&TickInputs {
        chunks_loaded: around
            .into_iter()
            .map(|position| (position, floor()))
            .collect(),
        chunks_kept: kept,
        ..TickInputs::default()
    });
    region
}

fn lit(region: &Region) -> usize {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .filter(|chunk| {
            region
                .light_at(BlockPos::new(chunk.x * 16, -60, chunk.z * 16))
                .is_some()
        })
        .count()
}

#[test]
fn a_budget_lights_the_chunks_nearest_the_players_first_a_step_at_a_time() {
    // 16 units: two chunks lit whole a step.
    let mut region = floor_of_25(Some(16), Vec::new());
    assert_eq!(lit(&region), 2);
    // The player stands at the origin, which is among the first.
    assert!(region.light_at(BlockPos::new(0, -60, 0)).is_some());
    idle_tick(&mut region);
    assert_eq!(lit(&region), 4);
    // No chunk ticks before it and its eight neighbours are lit.
    assert!(!region.ticks_chunk(ChunkPos::new(0, 0)));
    for _ in 0..20 {
        idle_tick(&mut region);
    }
    assert_eq!(lit(&region), 25);
    assert!(region.ticks_chunk(ChunkPos::new(0, 0)));
}

#[test]
fn without_a_budget_every_chunk_is_lit_and_ticks_in_the_tick_it_comes() {
    let region = floor_of_25(None, Vec::new());
    assert_eq!(lit(&region), 25);
    assert!(region.ticks_chunk(ChunkPos::new(0, 0)));
}

#[test]
fn light_handed_on_at_a_merge_is_adopted_at_one_unit_a_chunk() {
    let mut first = floor_of_25(None, Vec::new());
    let kept = first.hand_over_light();
    assert_eq!(kept.len(), 25);
    // The same chunks come back with their light: 16 units take all of them in two
    // steps (16 a step at one unit each), where lit whole they would take thirteen.
    let mut region = floor_of_25(Some(16), kept);
    assert_eq!(lit(&region), 16);
    idle_tick(&mut region);
    assert_eq!(lit(&region), 25);
    // And the light is the same as lighting them whole.
    let whole = floor_of_25(None, Vec::new());
    for x in -40..40 {
        let pos = BlockPos::new(x, -60, 3);
        assert_eq!(region.light_at(pos), whole.light_at(pos), "{pos:?}");
    }
}
