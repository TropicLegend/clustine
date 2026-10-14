//! Test 1 of `docs/adr/0021-generation-is-a-function.md`, section 9: the scheduler's
//! finished chunks against the plain model of section 2, and the toys that reach
//! beyond what a run may touch. Also the lattice of section 2, worked out here, against
//! the record's own figures, so that the counts the other tests expect stand on
//! something checked.

mod common;

use std::collections::BTreeSet;

use clustine_data::blocks;
use clustine_world::ChunkPos;
use clustine_worldgen::scheduler::{Options, Order, Scheduler};

use common::{
    FAR_MADE, FAR_SET, FAR_SOURCE, Kind, Mode, Model, PIECE_WITNESS, Toy, assert_finished_equal,
    distance, finished_all, finished_difference, finished_needs, lit_depends, lit_needs_of, rank,
    runs_of, view_area, within, wool,
};

/// Least, mean and most of a figure over the nine classes.
fn spread(figures: &[usize]) -> (usize, f64, usize) {
    let least = *figures.iter().min().unwrap();
    let most = *figures.iter().max().unwrap();
    let mean = figures.iter().sum::<usize>() as f64 / figures.len() as f64;
    (least, mean, most)
}

fn classes() -> impl Iterator<Item = ChunkPos> {
    (0..3).flat_map(|x| (0..3).map(move |z| ChunkPos::new(x, z)))
}

#[test]
fn the_lattice_has_the_figures_of_section_two() {
    // L1 and L2.
    for centre in classes() {
        let ranks: BTreeSet<u8> = within(centre, 1).map(rank).collect();
        assert_eq!(ranks.len(), 9, "the three by three around {centre:?}");
        for other in within(centre, 2) {
            if other != centre {
                assert_ne!(rank(other), rank(centre), "{other:?} beside {centre:?}");
            }
        }
    }

    // The runs a run of each rank depends on.
    let mut by_rank = [0; 9];
    for centre in classes() {
        by_rank[usize::from(rank(centre))] = runs_of(centre).len() - 1;
    }
    assert_eq!(by_rank, [0, 2, 5, 12, 20, 33, 48, 69, 93]);

    // One finished chunk, wherever it lies.
    for e in classes() {
        let needs = finished_needs(e);
        assert_eq!(
            (needs.runs.len(), needs.terrain.len(), needs.starts.len()),
            (94, 289, 1089),
            "finished at {e:?}"
        );
    }

    let table = |sets: Vec<common::Needs>| {
        let runs: Vec<_> = sets.iter().map(|n| n.runs.len()).collect();
        let terrain: Vec<_> = sets.iter().map(|n| n.terrain.len()).collect();
        let starts: Vec<_> = sets.iter().map(|n| n.starts.len()).collect();
        (spread(&runs), spread(&terrain), spread(&starts))
    };
    let close = |(least, mean, most): (usize, f64, usize), expected: (usize, usize, usize)| {
        least == expected.0 && (mean - expected.1 as f64).abs() < 1.0 && most == expected.2
    };
    // Each row of the table: what is lit around a chunk, and least, mean and most of
    // the runs, the terrain and the starts that needs.
    type Lit = Box<dyn Fn(ChunkPos) -> BTreeSet<ChunkPos>>;
    type Row = (&'static str, Lit, [(usize, usize, usize); 3]);
    let rows: [Row; 4] = [
        (
            "one lit chunk",
            Box::new(|p| BTreeSet::from([p])),
            [(94, 134, 157), (289, 361, 400), (1089, 1225, 1296)],
        ),
        (
            "a chunk and its eight neighbours lit",
            Box::new(|p| within(p, 1).collect()),
            [(157, 182, 238), (400, 441, 529), (1296, 1369, 1521)],
        ),
        (
            "a first view at distance 8",
            Box::new(|p| view_area(p, 8)),
            [(742, 822, 859), (1225, 1337, 1390), (2601, 2777, 2862)],
        ),
        (
            "the same with the ring made ahead",
            Box::new(|p| common::dilate(&view_area(p, 8), 1)),
            [(904, 942, 994), (1435, 1489, 1573), (2907, 2993, 3141)],
        ),
    ];
    for (row, lit, expected) in rows {
        let sets: Vec<_> = classes().map(|p| lit_needs_of(&lit(p))).collect();
        let (runs, terrain, starts) = table(sets);
        assert!(close(runs, expected[0]), "{row}: runs {runs:?}");
        assert!(close(terrain, expected[1]), "{row}: terrain {terrain:?}");
        assert!(close(starts, expected[2]), "{row}: starts {starts:?}");
    }

    // A first view: 329 chunks lit, 409 finished.
    let view = view_area(ChunkPos::new(0, 0), 8);
    assert_eq!(view.len(), 329);
    assert_eq!(common::dilate(&view, 1).len(), 409);
    assert_eq!(common::dilate(&common::dilate(&view, 1), 1).len(), 497);

    // Section 5: one failed run marks 25 to 289 lit chunks by its rank; a failed
    // terrain leaves 289 to 400 lit chunks unanswered, a failed start 1,225.
    let around = |n| {
        let centre = ChunkPos::new(0, 0);
        within(centre, n)
    };
    let mut marked = [0; 9];
    let mut unlit_by_terrain = Vec::new();
    for failed in classes() {
        marked[usize::from(rank(failed))] = around(20)
            .filter(|p| lit_depends(*p, Kind::Runs, failed))
            .count();
        unlit_by_terrain.push(
            around(20)
                .filter(|p| lit_depends(*p, Kind::Terrain, failed))
                .count(),
        );
    }
    assert_eq!(marked[0], 289);
    assert_eq!(marked[8], 25);
    assert_eq!(
        (
            *unlit_by_terrain.iter().min().unwrap(),
            *unlit_by_terrain.iter().max().unwrap()
        ),
        (289, 400)
    );
    let unlit_by_starts = around(30)
        .filter(|p| lit_depends(*p, Kind::Starts, ChunkPos::new(0, 0)))
        .count();
    assert_eq!(unlit_by_starts, 1225);
}

/// The rectangle of test 1: 30 by 30 chunks, away from the origin so that negative
/// coordinates are in it.
const MIN: ChunkPos = ChunkPos::new(-17, -11);
const MAX: ChunkPos = ChunkPos::new(12, 18);

fn scheduler(toy: Toy, order: Order) -> Scheduler<Toy> {
    let options = Options {
        order,
        ..common::options(4, false)
    };
    Scheduler::new(toy, options)
}

/// Compares the scheduler's finished chunks with the model run in two orders within
/// each rank, and gives back the chunks compared and the model's chunks.
fn against_the_model(
    seed: u64,
    mode: Mode,
    order: Order,
    min: ChunkPos,
    max: ChunkPos,
) -> (
    Scheduler<Toy>,
    Vec<ChunkPos>,
    std::collections::BTreeMap<ChunkPos, clustine_world::Chunk>,
    common::ModelCounts,
) {
    let model = Model::new(Toy::new(seed, mode), min, max);
    let answered = model.answered();
    assert!(
        answered.len() >= 25,
        "the rectangle answers for {} chunks",
        answered.len()
    );
    let (forwards, counts) = model.run(order, false);
    let (backwards, counts_backwards) = model.run(order, true);
    // How often a far read would have come out otherwise had it been answered live
    // depends on the order within a rank: that is why such reads are answered from
    // terrain. Nothing else may.
    let unordered = |counts: common::ModelCounts| common::ModelCounts {
        far_reads_live_would_change: 0,
        ..counts
    };
    assert_eq!(unordered(counts), unordered(counts_backwards));
    for e in &answered {
        // The model in its two orders agrees with itself: the toys keep to conditions
        // 1 to 3, or the comparison below would prove nothing.
        assert_finished_equal(*e, &forwards[e], &backwards[e], "the model's two orders");
    }

    let scheduler = scheduler(Toy::new(seed, mode), order);
    let finished = finished_all(&scheduler, &answered);
    for e in &answered {
        assert_finished_equal(
            *e,
            &finished[e],
            &forwards[e],
            "the scheduler against the model",
        );
    }
    scheduler.wait_idle();
    (scheduler, answered, forwards, counts)
}

#[test]
fn every_finished_chunk_equals_the_plain_model_in_the_order_of_nine() {
    // The order is something the toy can see: alone, the runs come out otherwise in
    // most chunks, so that the comparison below tells the two apart.
    let model = Model::new(Toy::new(11, Mode::Main), MIN, MAX);
    let answered = model.answered();
    let (nine, _) = model.run(Order::Nine, false);
    let (alone, _) = model.run(Order::Alone, false);
    let differ = answered
        .iter()
        .filter(|e| finished_difference(&nine[e], &alone[e]).is_some())
        .count();
    assert!(
        differ * 2 > answered.len(),
        "only {differ} of {} chunks differ between the two orders",
        answered.len()
    );

    let (scheduler, answered, _, counts) = against_the_model(11, Mode::Main, Order::Nine, MIN, MAX);
    assert!(answered.len() > 200, "{} chunks compared", answered.len());

    // The toy's features stay within what a run may touch, so nothing is recorded.
    let figures = scheduler.figures();
    assert_eq!(figures.recorded, Default::default(), "{figures:?}");
    assert_eq!(counts.far_reads + counts.refused_writes + counts.dropped, 0);
    assert_eq!((figures.failed_placements, figures.failed_tasks), (0, 0));
}

#[test]
fn every_finished_chunk_equals_the_plain_model_when_every_run_sees_terrain_alone() {
    let (scheduler, answered, _, _) = against_the_model(12, Mode::Main, Order::Alone, MIN, MAX);
    assert!(answered.len() > 200);
    assert_eq!(scheduler.figures().recorded, Default::default());
}

/// A smaller rectangle for the toys that reach too far: each is judged on its own.
const SMALL_MIN: ChunkPos = ChunkPos::new(-6, -4);
const SMALL_MAX: ChunkPos = ChunkPos::new(13, 15);

#[test]
fn a_live_read_seventeen_blocks_out_is_answered_from_terrain_and_counted() {
    for order in [Order::Nine, Order::Alone] {
        let (scheduler, _, _, counts) =
            against_the_model(21, Mode::ReadsFar, order, SMALL_MIN, SMALL_MAX);
        if order == Order::Nine {
            // Answered live, enough of these reads would have come out otherwise for
            // the comparison to tell.
            assert!(
                counts.far_reads_live_would_change * 4 > counts.runs,
                "{counts:?}"
            );
        }
        // One far read a run, nothing else recorded.
        let figures = scheduler.figures();
        assert!(figures.runs > 0);
        assert_eq!(figures.runs_made_again, 0);
        assert_eq!(
            figures.recorded.far_reads, figures.runs,
            "{order:?}: {figures:?}"
        );
        let recorded = clustine_worldgen::scheduler::Recorded {
            far_reads: 0,
            ..figures.recorded
        };
        assert_eq!(recorded, Default::default(), "{order:?}: {figures:?}");
    }
}

#[test]
fn writes_ticks_and_marks_seventeen_blocks_out_are_refused_dropped_and_counted() {
    for order in [Order::Nine, Order::Alone] {
        let (scheduler, answered, model, _) =
            against_the_model(22, Mode::WritesFar, order, SMALL_MIN, SMALL_MAX);
        // The run saw both writes refused: its witness says neither went through.
        for e in &answered {
            let witness = model[e].get(9, 150, 9);
            assert_eq!(witness, Some(wool(1)), "{e:?}");
        }
        // A block and a block entity refused, a tick and a mark dropped, every run.
        let figures = scheduler.figures();
        assert!(figures.runs > 0);
        assert_eq!(
            figures.recorded.refused_writes,
            2 * figures.runs,
            "{figures:?}"
        );
        assert_eq!(figures.recorded.dropped, 2 * figures.runs, "{figures:?}");
        assert_eq!(
            (
                figures.recorded.far_reads,
                figures.recorded.beyond_margin,
                figures.recorded.far_pieces,
                figures.recorded.far_piece_reads
            ),
            (0, 0, 0, 0)
        );
    }
}

/// What the witness of a far piece toy says the run at `c` read, from the finished
/// chunk at `c`.
fn witnessed(chunk: &clustine_world::Chunk) -> Option<u16> {
    let (x, y, z) = PIECE_WITNESS;
    let state = chunk.get(x, y, z)?;
    (state.0 >= blocks::WHITE_WOOL.0 && state.0 < blocks::WHITE_WOOL.0 + 16)
        .then(|| state.0 - blocks::WHITE_WOOL.0)
}

#[test]
fn a_piece_set_three_chunks_from_another_of_its_chunks_is_kept_and_counted() {
    // The piece spans chunks 0 to 3 in x; the run of (0, 0), of rank 0, sets it. The
    // runs of (1, 0) and (2, 0), of ranks 7 and 8 and within 2, read what it set; the
    // run of (3, 0), of rank 0 and three away, reads it as made, as definition D says.
    let scheduler = Scheduler::new(Toy::new(31, Mode::SetsFarPiece), common::options(4, false));
    let box_chunks: Vec<_> = (0..4).map(|x| ChunkPos::new(x, 0)).collect();
    let finished = finished_all(&scheduler, &box_chunks);
    let expected = [FAR_MADE, FAR_SET, FAR_SET, FAR_MADE];
    for (c, flags) in box_chunks.iter().zip(expected) {
        assert_eq!(
            witnessed(&finished[c]),
            Some(flags % 16),
            "the run of {c:?}"
        );
    }
    scheduler.wait_idle();
    let figures = scheduler.figures();
    // Only the run of (0, 0) sets it, and every run that reads it is one it meets.
    assert_eq!(figures.recorded.far_pieces, 1, "{figures:?}");
    assert_eq!(figures.recorded.far_piece_reads, 0, "{figures:?}");
    assert_eq!(
        (
            figures.recorded.far_reads,
            figures.recorded.refused_writes,
            figures.recorded.dropped
        ),
        (0, 0, 0)
    );
}

#[test]
fn a_piece_read_from_far_off_is_answered_and_counted() {
    // The piece is in (0, 0) alone and its run sets it after reading it. Every run
    // reads it: those within 2 of a higher rank see what it set, all others read it
    // as made, and those more than 2 away are counted.
    let scheduler = Scheduler::new(Toy::new(32, Mode::ReadsFarPiece), common::options(4, false));
    let asked = [
        ChunkPos::new(0, 0),
        ChunkPos::new(1, 1),
        ChunkPos::new(-2, 2),
        ChunkPos::new(2, 0),
        ChunkPos::new(3, 0),
        ChunkPos::new(-4, 1),
        ChunkPos::new(5, -5),
    ];
    let finished = finished_all(&scheduler, &asked);
    for c in asked {
        let expected = if c != FAR_SOURCE && distance(c, FAR_SOURCE) <= 2 {
            FAR_SET
        } else {
            FAR_MADE
        };
        assert_eq!(
            witnessed(&finished[&c]),
            Some(expected % 16),
            "the run of {c:?}"
        );
    }
    scheduler.wait_idle();
    let runs: BTreeSet<ChunkPos> = asked.iter().flat_map(|e| finished_needs(*e).runs).collect();
    let far = runs
        .iter()
        .filter(|c| distance(**c, FAR_SOURCE) > 2)
        .count() as u64;
    let figures = scheduler.figures();
    assert_eq!(figures.runs, runs.len() as u64, "{figures:?}");
    assert_eq!(figures.recorded.far_piece_reads, far, "{figures:?}");
    assert_eq!(figures.recorded.far_pieces, 0, "{figures:?}");
}
