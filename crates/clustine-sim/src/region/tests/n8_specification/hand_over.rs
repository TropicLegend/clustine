//! Light handed on with the chunks at a merge or a split (the plan's 4.5 and finding 1
//! of section 14, ADR-0029 section 7): the last tick's changes lit before the hand-over,
//! waiting chunks handed on without levels, the kept levels adopted and mended at their
//! faces towards chunks that come otherwise, in whichever order they come, and the light
//! then equal to lighting everything afresh.

use clustine_light::engine::{Chunks, Lighting};

use super::*;

/// The 5×5 about the origin.
fn square() -> Vec<ChunkPos> {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A region player 1 has joined, given viewers' tickets on `wanted` and then the floor
/// with `placed` there (and `kept` light for some), within `budget`.
fn loaded(
    area: ChunkArea,
    wanted: &[ChunkPos],
    placed: &[(BlockPos, BlockState)],
    kept: Vec<(ChunkPos, crate::KeptLight)>,
    budget: Option<u32>,
) -> Region {
    let mut region = joined_in(area, &[1]);
    region.config.light.budget = budget;
    region.tick(&tickets(wanted.to_vec(), vec![]));
    region.tick(&TickInputs {
        chunks_loaded: wanted.iter().map(|at| (*at, laid(*at, placed))).collect(),
        chunks_kept: kept,
        ..TickInputs::default()
    });
    region
}

/// The floor of the chunk at `position` with those of `placed` that are in it.
fn laid(position: ChunkPos, placed: &[(BlockPos, BlockState)]) -> Chunk {
    let mut chunk = floor();
    for (pos, state) in placed.iter().filter(|(pos, _)| pos.chunk() == position) {
        let (x, z) = pos.in_chunk();
        chunk.set(x, pos.y, z, *state);
    }
    chunk
}

/// Asserts that `region` has the light of `whole` at every block of the ground and the
/// air above it in the chunks `compared`.
fn same_light(region: &Region, whole: &Region, compared: &[ChunkPos]) {
    for chunk in compared {
        for x in 0..16 {
            for z in 0..16 {
                for y in [GROUND - 1, GROUND, GROUND + 1, GROUND + 4] {
                    let pos = at(chunk.x * 16 + x, y, chunk.z * 16 + z);
                    assert_eq!(levels(region, pos), levels(whole, pos), "{pos:?}");
                }
            }
        }
    }
}

/// Lights by a face and in a corner: glowstone at the face of (0, 0) towards (1, 0),
/// a torch at the corner where four chunks meet, and a tunnel of glass.
fn lights() -> Vec<(BlockPos, BlockState)> {
    vec![
        (at(15, GROUND, 7), blocks::GLOWSTONE),
        (at(-1, GROUND, -1), blocks::TORCH),
        (at(16, GROUND + 1, -3), blocks::GLASS),
        (at(20, GROUND + 3, 4), blocks::STONE),
    ]
}

#[test]
fn a_change_after_the_last_step_is_lit_in_the_light_handed_on() {
    let mut donor = loaded(ChunkArea::EVERYWHERE, &square(), &[], Vec::new(), None);
    // Set after the tick's light step, as a block tick or a random tick sets it, and
    // so not lit before the hand-over runs the step once more.
    for (pos, placed) in lights() {
        put(&mut donor, pos, placed);
    }
    let handed = donor.hand_over_light();
    assert_eq!(handed.len(), 25);
    let chunks: Vec<(ChunkPos, Chunk)> = square()
        .into_iter()
        .map(|at| (at, donor.chunk(at).expect("loaded").clone()))
        .collect();
    let mut taker = joined_in(ChunkArea::EVERYWHERE, &[1]);
    taker.tick(&tickets(square(), vec![]));
    taker.tick(&TickInputs {
        chunks_loaded: chunks,
        chunks_kept: handed,
        ..TickInputs::default()
    });
    let whole = loaded(
        ChunkArea::EVERYWHERE,
        &square(),
        &lights(),
        Vec::new(),
        None,
    );
    assert_eq!(
        levels(&whole, at(15, GROUND, 7)).map(|(_, block)| block),
        Some(15)
    );
    assert_eq!(
        levels(&whole, at(16, GROUND, 7)).map(|(_, block)| block),
        Some(14)
    );
    same_light(&taker, &whole, &square());
}

/// What the engine reads: the chunks of a region.
struct Of<'a>(&'a Region);

impl Chunks for Of<'_> {
    fn chunk(&self, at: ChunkPos) -> Option<&Chunk> {
        self.0.chunk(at)
    }
}

#[test]
fn chunks_handed_back_with_their_light_are_adopted_and_none_is_lit_whole() {
    let donor = loaded(
        ChunkArea::EVERYWHERE,
        &square(),
        &lights(),
        Vec::new(),
        None,
    );
    let mut donor = donor;
    let handed = donor.hand_over_light();
    let mut lighting = Lighting::new(true);
    for (at, kept) in handed {
        lighting.adopt(at, kept);
    }
    let work = lighting.run(&Of(&donor));
    assert_eq!(work.chunks_lit_whole, 0, "{work:?}");
    assert_eq!(work.adopted, 25, "{work:?}");
    for at in square() {
        let made = lighting.light(at, &Of(&donor));
        let near = [
            ChunkPos::new(at.x - 1, at.z - 1),
            ChunkPos::new(at.x, at.z - 1),
            ChunkPos::new(at.x + 1, at.z - 1),
            ChunkPos::new(at.x - 1, at.z),
            ChunkPos::new(at.x + 1, at.z),
            ChunkPos::new(at.x - 1, at.z + 1),
            ChunkPos::new(at.x, at.z + 1),
            ChunkPos::new(at.x + 1, at.z + 1),
        ];
        let expected = clustine_light::light(&clustine_light::Around {
            centre: donor.chunk(at).expect("loaded"),
            neighbours: near.map(|near| donor.chunk(near)),
            sky: true,
        });
        assert!(made == Some(expected), "{at:?}");
    }
}

#[test]
fn chunks_still_waiting_at_a_hand_over_are_handed_on_without_light() {
    // Three chunks lit whole a step: after the tick they come, three are lit.
    let mut donor = loaded(ChunkArea::EVERYWHERE, &square(), &[], Vec::new(), Some(24));
    let handed = donor.hand_over_light();
    let mut taken: Vec<ChunkPos> = handed.iter().map(|(at, _)| *at).collect();
    taken.sort();
    let mut expected = vec![
        ChunkPos::new(0, 0),
        ChunkPos::new(-1, -1),
        ChunkPos::new(-1, 0),
    ];
    expected.sort();
    assert_eq!(taken, expected);
}

#[test]
fn kept_light_beside_a_chunk_that_comes_afresh_with_other_blocks_is_mended_at_the_face() {
    // The donor had plain floors; the chunk east of the origin comes back from the
    // store afresh with glowstone at its western face, and the one north of it with a
    // roof over its half.
    let mut donor = loaded(ChunkArea::EVERYWHERE, &square(), &[], Vec::new(), None);
    let fresh = [ChunkPos::new(1, 0), ChunkPos::new(0, -1)];
    let kept: Vec<(ChunkPos, crate::KeptLight)> = donor
        .hand_over_light()
        .into_iter()
        .filter(|(at, _)| !fresh.contains(at))
        .collect();
    assert_eq!(kept.len(), 23);
    let mut changed = vec![(at(16, GROUND, 7), blocks::GLOWSTONE)];
    changed.extend(
        (0..16).flat_map(|x| (-16..-8).map(move |z| (at(x, GROUND + 2, z), blocks::STONE))),
    );
    let region = loaded(ChunkArea::EVERYWHERE, &square(), &changed, kept, None);
    let whole = loaded(ChunkArea::EVERYWHERE, &square(), &changed, Vec::new(), None);
    assert_eq!(
        levels(&region, at(15, GROUND, 7)).map(|(_, block)| block),
        Some(14)
    );
    same_light(&region, &whole, &square());
}

#[test]
fn kept_light_adopted_a_few_chunks_a_step_ends_as_the_light_lit_whole() {
    let mut donor = loaded(
        ChunkArea::EVERYWHERE,
        &square(),
        &lights(),
        Vec::new(),
        None,
    );
    let handed = donor.hand_over_light();
    // Five units: five chunks adopted a step, so neighbours from the same hand-over are
    // adopted steps apart, each one's faces mended towards the others as they come.
    let mut region = loaded(ChunkArea::EVERYWHERE, &square(), &lights(), handed, Some(5));
    for _ in 0..5 {
        idle(&mut region);
    }
    let whole = loaded(
        ChunkArea::EVERYWHERE,
        &square(),
        &lights(),
        Vec::new(),
        None,
    );
    same_light(&region, &whole, &square());
}

/// The west of a split at the line x = 16: the chunks from x = −2 to 0 loaded with the
/// light `kept`, and the column at x = 1 copied from `beyond`, the copies before the
/// loaded chunks or after them.
fn west_of_a_split(
    kept: Vec<(ChunkPos, crate::KeptLight)>,
    placed: &[(BlockPos, BlockState)],
    copies_first: bool,
) -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };
    let own: Vec<ChunkPos> = square().into_iter().filter(|at| at.x <= 0).collect();
    let copied: Vec<ChunkPos> = (-2..=2).map(|z| ChunkPos::new(1, z)).collect();
    let mut region = joined_in(west, &[1]);
    region.tick(&tickets(own.clone(), vec![]));
    let copies = TickInputs {
        foreign: copied.iter().map(|at| (*at, RegionId(1))).collect(),
        copy_bases: copied.iter().map(|at| (*at, laid(*at, placed))).collect(),
        ..TickInputs::default()
    };
    if copies_first {
        region.tick(&copies);
    }
    let kept: Vec<(ChunkPos, crate::KeptLight)> = kept
        .into_iter()
        .filter(|(at, _)| own.contains(at))
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: own.iter().map(|at| (*at, laid(*at, placed))).collect(),
        chunks_kept: kept,
        ..TickInputs::default()
    });
    if !copies_first {
        region.tick(&copies);
    }
    idle(&mut region);
    region
}

#[test]
fn the_west_of_a_split_has_the_light_of_the_whole_whether_its_copies_come_before_or_after() {
    // Glowstone east of the line, in what becomes a copy, lights the west's own chunk.
    let mut placed = lights();
    placed.push((at(17, GROUND, 12), blocks::GLOWSTONE));
    let mut donor = loaded(ChunkArea::EVERYWHERE, &square(), &placed, Vec::new(), None);
    let handed = donor.hand_over_light();
    let whole = loaded(ChunkArea::EVERYWHERE, &square(), &placed, Vec::new(), None);
    assert_eq!(
        levels(&whole, at(15, GROUND, 12)).map(|(_, block)| block),
        Some(13)
    );
    let compared: Vec<ChunkPos> = square().into_iter().filter(|at| at.x <= 0).collect();
    for copies_first in [true, false] {
        let west = west_of_a_split(handed.clone(), &placed, copies_first);
        assert!(west.copies.contains_key(&ChunkPos::new(1, 0)));
        same_light(&west, &whole, &compared);
    }
}
