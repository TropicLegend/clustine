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

/// Hides from the engine a glowstone set in the chunk at the origin, as a write that went
/// round `Chunk::set`'s noting would: the block is there and its change is not noted.
fn glowstone_hidden_from_the_engine(region: &mut Region) {
    let chunk = region
        .chunks
        .get_mut(&ChunkPos::new(0, 0))
        .expect("the origin's chunk is loaded");
    chunk.set(8, -59, 8, blocks::GLOWSTONE);
    let _ = chunk.take_light_changes();
}

/// The plan of N8, section 4.7: in the simulation's own tests every chunk a step could
/// have changed the light of is checked, so a change the engine was not told of is
/// found where it is lit again. Here a change noted beside it relights the chunk.
#[test]
#[should_panic(expected = "differs from the function's of its lit 3×3")]
fn a_change_the_engine_was_not_told_of_is_found_by_the_check() {
    let mut region = floor_of_25(None, Vec::new());
    assert_eq!(region.config.light.checks, crate::LightChecks::Touched);
    glowstone_hidden_from_the_engine(&mut region);
    // A noted change in the same chunk: the step checks the chunk, whose light the
    // engine relights only about the noted block.
    region
        .chunks
        .get_mut(&ChunkPos::new(0, 0))
        .expect("loaded")
        .set(1, -59, 1, blocks::STONE);
    idle_tick(&mut region);
}

/// The plan of N8, section 4.7: the end-to-end tests' checks of a few chunks a step come
/// round to every lit chunk, so that a difference that lasts is found within a turn.
#[test]
#[should_panic(expected = "differs from the function's of its lit 3×3")]
fn checks_in_turn_come_round_to_a_lasting_difference() {
    let mut region = floor_of_25(None, Vec::new());
    region.config.light.checks = crate::LightChecks::InTurn(4);
    glowstone_hidden_from_the_engine(&mut region);
    // 25 chunks at 4 a step: within seven steps every one is checked.
    for _ in 0..7 {
        idle_tick(&mut region);
    }
}

/// The plan of N8, section 4.7: light that agrees passes the checks in turn, step after
/// step, round and round the region's chunks.
#[test]
fn light_that_agrees_passes_the_checks_in_turn() {
    let mut region = floor_of_25(None, Vec::new());
    region.config.light.checks = crate::LightChecks::InTurn(4);
    region
        .chunks
        .get_mut(&ChunkPos::new(0, 0))
        .expect("loaded")
        .set(8, -59, 8, blocks::GLOWSTONE);
    for _ in 0..20 {
        idle_tick(&mut region);
    }
}
