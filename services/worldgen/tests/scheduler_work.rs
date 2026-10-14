//! Tests 2 to 5 of `docs/adr/0021-generation-is-a-function.md`, section 9: the order
//! of work and the cache never show in a chunk, a request makes exactly the sets of
//! section 2, and the look-ahead and cancelling of section 5.

mod common;

use std::collections::BTreeSet;
use std::sync::mpsc;

use clustine_world::{ChunkGenerator, ChunkPos};
use clustine_worldgen::scheduler::{Figures, Options, Pick, Scheduler};

use common::{
    Gate, Mode, TOY_VERSION, Toy, finished_needs, lit_all, lit_needs, lit_needs_of, view_area,
    within,
};

/// The lit chunks of a few requests that overlap and a few that stand alone.
fn requested() -> BTreeSet<ChunkPos> {
    let mut positions: BTreeSet<_> = within(ChunkPos::new(2, -1), 2).collect();
    positions.extend([
        ChunkPos::new(20, 7),
        ChunkPos::new(-13, 4),
        ChunkPos::new(-14, 4),
    ]);
    positions
}

fn made_with(seed: u64, options: Options) -> Vec<(ChunkPos, clustine_world::Chunk)> {
    let scheduler = Scheduler::new(Toy::new(seed, Mode::Main), options);
    let made = lit_all(&scheduler, requested());
    scheduler.wait_idle();
    made.into_iter()
        .map(|(position, chunk)| {
            let chunk = chunk.unwrap_or_else(|_| panic!("{position:?} failed"));
            (position, chunk)
        })
        .collect()
}

#[test]
fn the_order_of_work_does_not_show_in_the_chunks() {
    let one = made_with(41, common::options(1, true));
    for (position, chunk) in &one {
        assert_eq!(chunk.generated(), TOY_VERSION, "{position:?}");
        assert!(!chunk.with_failure(), "{position:?}");
        assert!(chunk.light().is_some(), "{position:?} is lit");
    }
    let others = [
        ("eight threads", common::options(8, true)),
        (
            "one thread, scrambled",
            Options {
                pick: Pick::Shuffled(0x5eed),
                ..common::options(1, true)
            },
        ),
        (
            "eight threads, scrambled",
            Options {
                pick: Pick::Shuffled(99),
                ..common::options(8, true)
            },
        ),
    ];
    for (what, options) in others {
        let made = made_with(41, options);
        assert_eq!(made.len(), one.len());
        for ((position, chunk), (_, expected)) in made.iter().zip(&one) {
            assert!(
                chunk == expected,
                "{what}: the lit chunk at {position:?} differs"
            );
        }
    }
}

#[test]
fn the_cache_does_not_show_in_the_chunks() {
    let unbounded = made_with(42, common::options(4, true));
    for bound in [1024 * 1024, 0] {
        let options = Options {
            cache_bytes: Some(bound),
            ..common::options(4, true)
        };
        let made = made_with(42, options.clone());
        for ((position, chunk), (_, expected)) in made.iter().zip(&unbounded) {
            assert!(
                chunk == expected,
                "a bound of {bound}: {position:?} differs"
            );
        }

        // One after another, so that what one request made is dropped before the
        // next needs it, and is made again.
        let scheduler = Scheduler::new(Toy::new(42, Mode::Main), options);
        for (position, expected) in unbounded.iter().rev().step_by(5) {
            let chunk = scheduler.generate(*position);
            assert!(
                chunk == *expected,
                "a bound of {bound}, one by one: {position:?} differs"
            );
        }
    }
}

#[test]
fn with_a_cache_of_nothing_a_request_under_way_loses_nothing_it_needs() {
    let options = Options {
        cache_bytes: Some(0),
        ..common::options(4, false)
    };
    let scheduler = Scheduler::new(Toy::new(43, Mode::Main), options);
    let position = ChunkPos::new(7, 5);
    // The request ends, and nothing it needed was made twice on its way.
    let chunk = scheduler.generate(position);
    scheduler.wait_idle();
    let needs = lit_needs(position);
    let figures = scheduler.figures();
    assert_eq!(figures.runs_made_again, 0, "{figures:?}");
    assert_eq!(figures.runs, needs.runs.len() as u64, "{figures:?}");
    assert_eq!(figures.terrain, needs.terrain.len() as u64, "{figures:?}");
    assert_eq!(figures.starts, needs.starts.len() as u64, "{figures:?}");
    assert_eq!((figures.queued, figures.running), (0, 0));

    let unbounded = Scheduler::new(Toy::new(43, Mode::Main), common::options(4, false));
    assert!(chunk == unbounded.generate(position));
}

/// The task counts by kind.
fn tasks(figures: &Figures) -> [u64; 5] {
    [
        figures.starts,
        figures.terrain,
        figures.runs,
        figures.finished,
        figures.lit,
    ]
}

#[test]
fn one_finished_chunk_from_nothing_makes_exactly_its_runs_terrain_and_starts() {
    for x in 0..3 {
        for z in 0..3 {
            let e = ChunkPos::new(x + 30, z - 61);
            let toy = Toy::new(44, Mode::Main);
            let calls = toy.calls.clone();
            let scheduler = Scheduler::new(toy, common::options(4, false));
            scheduler.finished(e).expect("the toy does not fail");
            scheduler.wait_idle();
            let figures = scheduler.figures();
            assert_eq!(tasks(&figures), [1089, 289, 94, 1, 0], "{e:?}: {figures:?}");
            assert_eq!(figures.runs_made_again, 0);
            // And the stages were asked for exactly those, once each.
            let (starts, terrain, runs, _) = calls.get();
            assert_eq!((starts, terrain, runs), (1089, 289, 94), "{e:?}");
            let needs = finished_needs(e);
            assert_eq!(
                (needs.runs.len(), needs.terrain.len(), needs.starts.len()),
                (94, 289, 1089)
            );
        }
    }
}

#[test]
fn a_first_view_makes_counts_within_the_table_of_section_two() {
    for centre in [
        ChunkPos::new(0, 0),
        ChunkPos::new(10, -5),
        ChunkPos::new(-7, 8),
    ] {
        let view = view_area(centre, 8);
        assert_eq!(view.len(), 329);
        let toy = Toy::new(45, Mode::Main);
        let calls = toy.calls.clone();
        let scheduler = Scheduler::new(toy, common::options(4, false));
        let made = lit_all(&scheduler, view.iter().copied());
        assert!(made.values().all(Result::is_ok));
        scheduler.wait_idle();
        let figures = scheduler.figures();
        let [starts, terrain, runs, finished, lit] = tasks(&figures);
        assert_eq!((finished, lit), (409, 329), "{centre:?}: {figures:?}");
        assert!((742..=859).contains(&runs), "{centre:?}: {runs} runs");
        assert!(
            (1225..=1390).contains(&terrain),
            "{centre:?}: {terrain} terrain"
        );
        assert!(
            (2601..=2862).contains(&starts),
            "{centre:?}: {starts} starts"
        );
        // Exactly the sets of section 2 for where the view lies.
        let needs = lit_needs_of(&view);
        assert_eq!(
            (runs, terrain, starts),
            (
                needs.runs.len() as u64,
                needs.terrain.len() as u64,
                needs.starts.len() as u64
            ),
            "{centre:?}"
        );
        let (called_starts, called_terrain, called_runs, _) = calls.get();
        assert_eq!(
            (called_starts, called_terrain, called_runs),
            (starts, terrain, runs)
        );
    }
}

#[test]
fn the_ring_around_a_request_is_made_ahead_and_asking_for_it_runs_no_task() {
    let (gate, _entered) = Gate::new(true);
    let toy = Toy::new(46, Mode::Main).with_gate(gate.clone());
    let calls = toy.calls.clone();
    let scheduler = Scheduler::new(toy, common::options(2, true));
    let p = ChunkPos::new(4, -7);
    scheduler.generate(p);
    scheduler.wait_idle();

    // The request and its eight neighbours lit, and what that needs, no more.
    let ring: BTreeSet<_> = within(p, 1).collect();
    let needs = lit_needs_of(&ring);
    let ahead = scheduler.figures();
    assert_eq!(
        tasks(&ahead),
        [
            needs.starts.len() as u64,
            needs.terrain.len() as u64,
            needs.runs.len() as u64,
            25,
            9
        ],
        "{ahead:?}"
    );
    assert!(ahead.cache_bytes > 0);

    // A neighbour is asked for while every stage is held: it is answered from the
    // cache. The look-ahead then wants the ring around it, none of which can be lit
    // from what is cached without a stage, so no lit chunk is made meanwhile.
    let q = ChunkPos::new(p.x + 1, p.z);
    let cached_runs = needs.runs.clone();
    let finished_cached = |e: ChunkPos| {
        ((e.x - p.x).abs() <= 2 && (e.z - p.z).abs() <= 2)
            || within(e, 1).all(|d| cached_runs.contains(&d))
    };
    let new_ring: Vec<_> = within(q, 1).filter(|n| !ring.contains(n)).collect();
    assert_eq!(new_ring.len(), 3);
    assert!(
        new_ring.iter().all(|n| !within(*n, 1).all(finished_cached)),
        "a premise of the test: the new ring needs a stage"
    );
    gate.set(false);
    let before = calls.get();
    let (send, answer) = mpsc::channel();
    let wanted = scheduler.request(
        q,
        Box::new(move |made| {
            let _ = send.send(made);
        }),
    );
    let made = answer
        .recv()
        .expect("answered")
        .expect("a chunk made ahead is answered");
    assert_eq!(
        scheduler.figures().lit,
        ahead.lit,
        "Lit({q:?}) was made again"
    );
    assert_eq!(calls.get(), before);
    gate.set(true);
    scheduler.wait_idle();
    drop(wanted);

    // What was made ahead is what a request makes.
    let fresh = Scheduler::new(Toy::new(46, Mode::Main), common::options(2, false));
    assert!(made == fresh.generate(q));
    // And the look-ahead went on to the ring around the neighbour.
    assert_eq!(scheduler.figures().lit, 9 + 3);
}

#[test]
fn a_dropped_request_leaves_no_task_waiting_and_what_ran_is_used_next() {
    let (gate, entered) = Gate::new(false);
    let toy = Toy::new(47, Mode::Main).with_gate(gate.clone());
    let calls = toy.calls.clone();
    let scheduler = Scheduler::new(toy, common::options(2, false));
    let p = ChunkPos::new(-3, 5);
    let wanted = scheduler.request(p, Box::new(|_| {}));

    // Both threads are in a task, held at the gate; the rest waits.
    entered.recv().expect("a task starts");
    entered.recv().expect("a second task starts");
    let held = scheduler.figures();
    assert_eq!(held.running, 2, "{held:?}");
    assert!(held.queued > 0, "{held:?}");

    drop(wanted);
    gate.set(true);
    scheduler.wait_idle();
    // The two under way ended; nothing else was started.
    let figures = scheduler.figures();
    assert_eq!(tasks(&figures), [2, 0, 0, 0, 0], "{figures:?}");
    assert_eq!((figures.queued, figures.running), (0, 0));
    assert_eq!(calls.get().0, 2);

    // The next request uses what they made.
    let chunk = scheduler.generate(p);
    scheduler.wait_idle();
    let needs = lit_needs(p);
    let figures = scheduler.figures();
    assert_eq!(figures.starts, needs.starts.len() as u64, "{figures:?}");
    assert_eq!(calls.get().0, needs.starts.len() as u64);
    let fresh = Scheduler::new(Toy::new(47, Mode::Main), common::options(2, false));
    assert!(chunk == fresh.generate(p));
}
