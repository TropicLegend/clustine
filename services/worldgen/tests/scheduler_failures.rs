//! Test 6 of `docs/adr/0021-generation-is-a-function.md`, section 9: a placement that
//! fails is undone and marked, a panic between placements cuts the run short, a
//! failure is logged, and a failed terrain or start leaves exactly the chunks of
//! section 5's table unanswered until the scheduler is made again.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, Once};

use clustine_data::blocks;
use clustine_world::{Chunk, ChunkPos, InChunk};
use clustine_worldgen::scheduler::Scheduler;
use clustine_worldgen::stages::Stages;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use common::{
    AFTER_AT, BEFORE_AT, FAULT_Y, FAULTY_AT, FAULTY_CHEST_AT, Fault, Kind, Mode, TOY_VERSION, Toy,
    assert_finished_equal, dilate, finished_all, lit_all, lit_depends, rank, within,
};

// Capturing the log. The scheduler logs on threads of its own, so the subscriber is
// the process's; every test here filters what it finds by its toy's identity.

static LINES: Mutex<Vec<BTreeMap<String, String>>> = Mutex::new(Vec::new());

struct Capture;

struct Fields(BTreeMap<String, String>);

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        if *event.metadata().level() != tracing::Level::ERROR {
            return;
        }
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        LINES.lock().unwrap().push(fields.0);
    }
}

fn capture() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Capture))
            .expect("nothing else installs a subscriber in this test binary");
    });
}

/// The error lines logged for the toy of this identity.
fn lines_of(identity: &str) -> Vec<BTreeMap<String, String>> {
    LINES
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.get("identity").map(|i| i.trim_matches('"')) == Some(identity))
        .cloned()
        .collect()
}

fn ok(
    made: BTreeMap<ChunkPos, Result<Chunk, clustine_world::generation::Failed>>,
) -> BTreeMap<ChunkPos, Chunk> {
    made.into_iter()
        .map(|(position, chunk)| {
            (
                position,
                chunk.unwrap_or_else(|_| panic!("{position:?} failed")),
            )
        })
        .collect()
}

/// The chunk as it would be without its failure mark.
fn unmarked(chunk: &Chunk) -> Chunk {
    let mut chunk = chunk.clone();
    chunk.mark_generated(chunk.generated(), false);
    chunk
}

fn at(chunk: &Chunk, (x, z): (usize, usize), y: i32) -> clustine_data::BlockState {
    chunk.get(x, y, z).expect("within the height")
}

#[test]
fn a_placement_that_fails_is_undone_and_the_world_goes_on_without_it() {
    capture();
    // Rank 3: runs of lower and of higher rank are around it.
    let f = ChunkPos::new(13, -22);
    assert_eq!(rank(f), 3);
    let failing = Scheduler::new(
        Toy::new(61, Mode::Main).with_fault(f, Fault::Panics),
        common::options(4, false),
    );
    let without = Scheduler::new(
        Toy::new(61, Mode::Main).with_fault(f, Fault::DoesNothing),
        common::options(4, false),
    );
    let window: Vec<_> = within(f, 3).collect();
    let got = finished_all(&failing, &window);
    let expected = finished_all(&without, &window);
    for e in &window {
        assert_finished_equal(*e, &got[e], &expected[e], "failed against left out");
    }

    // The placements before and after it are there; nothing of it is.
    let centre = &got[&f];
    assert_eq!(at(centre, BEFORE_AT, FAULT_Y), blocks::SAND);
    assert_eq!(at(centre, AFTER_AT, FAULT_Y), blocks::GLASS_PANE);
    assert_eq!(at(centre, FAULTY_AT, FAULT_Y), blocks::AIR);
    assert_eq!(at(centre, FAULTY_CHEST_AT, FAULT_Y), blocks::AIR);
    let east = &got[&ChunkPos::new(f.x + 1, f.z)];
    assert_eq!(at(east, FAULTY_AT, FAULT_Y), blocks::AIR);
    let chest = InChunk::new(FAULTY_CHEST_AT.0, FAULT_Y, FAULTY_CHEST_AT.1);
    let faulty = InChunk::new(FAULTY_AT.0, FAULT_Y, FAULTY_AT.1);
    assert!(!centre.block_entities().contains_key(&chest));
    assert!(!centre.marks().contains(&chest));
    assert!(centre.ticks().iter().all(|tick| tick.at != faulty));

    // The lit chunks are the same but for the mark, which every one that depends on
    // the run carries.
    let around: Vec<_> = within(f, 1).collect();
    let lit = ok(lit_all(&failing, around.iter().copied()));
    let lit_without = ok(lit_all(&without, around.iter().copied()));
    for p in &around {
        assert!(lit[p].with_failure(), "{p:?} depends on the failed run");
        assert!(!lit_without[p].with_failure(), "{p:?}");
        assert_eq!(lit[p].generated(), TOY_VERSION);
        assert!(
            unmarked(&lit[p]) == lit_without[p],
            "the lit chunk at {p:?}"
        );
    }
    failing.wait_idle();
    let figures = failing.figures();
    assert_eq!(
        (figures.failed_placements, figures.failed_tasks),
        (1, 0),
        "{figures:?}"
    );
}

/// Exactly the lit chunks whose `R` holds the failed run are marked.
fn marks_exactly_those_that_depend_on(f: ChunkPos, seed: u64, expected: usize) {
    capture();
    let marked: BTreeSet<_> = within(f, 12)
        .filter(|p| lit_depends(*p, Kind::Runs, f))
        .collect();
    assert_eq!(marked.len(), expected);
    let scheduler = Scheduler::new(
        Toy::new(seed, Mode::Main).with_fault(f, Fault::Panics),
        common::options(4, false),
    );
    let asked = dilate(&marked, 1);
    let lit = ok(lit_all(&scheduler, asked.iter().copied()));
    for (p, chunk) in &lit {
        assert_eq!(chunk.generated(), TOY_VERSION, "{p:?}");
        assert_eq!(
            chunk.with_failure(),
            marked.contains(p),
            "{p:?}, {} from the failed run",
            common::distance(*p, f)
        );
    }
}

#[test]
fn a_failed_placement_in_a_run_of_rank_eight_marks_twenty_five_chunks() {
    let f = ChunkPos::new(5, 3);
    assert_eq!(rank(f), 8);
    marks_exactly_those_that_depend_on(f, 62, 25);
}

#[test]
fn a_failed_placement_in_a_run_of_rank_zero_marks_two_hundred_and_eighty_nine_chunks() {
    let f = ChunkPos::new(6, 3);
    assert_eq!(rank(f), 0);
    marks_exactly_those_that_depend_on(f, 63, 289);
}

#[test]
fn a_panic_between_two_placements_cuts_the_run_short_and_keeps_what_was_placed() {
    capture();
    let f = ChunkPos::new(-8, 10);
    let cut = Scheduler::new(
        Toy::new(64, Mode::Main).with_fault(f, Fault::CutShort),
        common::options(4, false),
    );
    let stopped = Scheduler::new(
        Toy::new(64, Mode::Main).with_fault(f, Fault::StopsEarly),
        common::options(4, false),
    );
    let window: Vec<_> = within(f, 2).collect();
    let got = finished_all(&cut, &window);
    let expected = finished_all(&stopped, &window);
    for e in &window {
        assert_finished_equal(*e, &got[e], &expected[e], "cut short against stopped");
    }
    assert_eq!(at(&got[&f], BEFORE_AT, FAULT_Y), blocks::SAND);
    assert_eq!(at(&got[&f], AFTER_AT, FAULT_Y), blocks::AIR);

    let lit = ok(lit_all(&cut, [f]));
    assert!(lit[&f].with_failure());
    let lit_stopped = ok(lit_all(&stopped, [f]));
    assert!(!lit_stopped[&f].with_failure());
    assert!(unmarked(&lit[&f]) == lit_stopped[&f]);
    cut.wait_idle();
    // The run gave its result: no task failed.
    assert_eq!(cut.figures().failed_tasks, 0);
}

#[test]
fn a_failure_is_logged_with_the_seed_the_chunk_what_was_placed_and_the_version() {
    capture();
    let f = ChunkPos::new(13, -22);
    let toy = Toy::new(65, Mode::Main).with_fault(f, Fault::Panics);
    let identity = toy.settings().identity;
    assert!(identity.contains("seed 65"));
    let scheduler = Scheduler::new(toy, common::options(2, false));
    scheduler
        .finished(f)
        .expect("a failed placement fails no chunk");
    scheduler.wait_idle();

    let lines: Vec<_> = lines_of(&identity)
        .into_iter()
        .filter(|line| {
            line.get("placing")
                .is_some_and(|p| p.contains("toy:faulty"))
        })
        .collect();
    assert_eq!(lines.len(), 1, "{:?}", lines_of(&identity));
    let line = &lines[0];
    assert_eq!(
        line.get("version").map(String::as_str),
        Some("7"),
        "{line:?}"
    );
    let chunk = line.get("chunk").expect("the line names the chunk");
    assert!(chunk.contains("13") && chunk.contains("-22"), "{line:?}");
    let message = line
        .get("message")
        .expect("the line has the panic's message");
    assert!(message.contains("the faulty placement fails"), "{line:?}");
}

/// Asks for every lit chunk around those that `fails` says depend on what panics, and
/// checks that exactly those are answered `Failed`, that asking again runs nothing, and
/// that a new scheduler tries again.
fn fails_exactly(toy: Toy, unlit: BTreeSet<ChunkPos>) {
    capture();
    let again = toy.again();
    let calls = toy.calls.clone();
    let scheduler = Scheduler::new(toy, common::options(4, false));
    let asked = dilate(&unlit, 1);
    let made = lit_all(&scheduler, asked.iter().copied());
    for (p, chunk) in &made {
        match chunk {
            Err(_) => assert!(unlit.contains(p), "{p:?} failed"),
            Ok(chunk) => {
                assert!(!unlit.contains(p), "{p:?} was made");
                assert!(!chunk.with_failure(), "{p:?}");
            }
        }
    }
    scheduler.wait_idle();
    assert_eq!(calls.get().3, 1, "the stage that panics was called once");
    let figures = scheduler.figures();
    assert!(figures.failed_tasks > 0, "{figures:?}");

    // The failure is remembered: the same request runs nothing.
    let q = *unlit.iter().next().unwrap();
    let before = (calls.get(), figures);
    assert!(lit_all(&scheduler, [q])[&q].is_err());
    scheduler.wait_idle();
    let after = scheduler.figures();
    assert_eq!(calls.get(), before.0);
    assert_eq!(
        [
            after.starts,
            after.terrain,
            after.runs,
            after.finished,
            after.lit
        ],
        [
            before.1.starts,
            before.1.terrain,
            before.1.runs,
            before.1.finished,
            before.1.lit
        ]
    );

    // A scheduler made anew tries again.
    let calls = again.calls.clone();
    let anew = Scheduler::new(again, common::options(4, false));
    assert!(lit_all(&anew, [q])[&q].is_err());
    assert_eq!(calls.get().3, 1);
}

#[test]
fn a_terrain_that_panics_fails_exactly_the_chunks_that_need_it() {
    let t = ChunkPos::new(2, -1);
    let unlit: BTreeSet<_> = within(t, 12)
        .filter(|p| lit_depends(*p, Kind::Terrain, t))
        .collect();
    assert!((289..=400).contains(&unlit.len()), "{}", unlit.len());
    fails_exactly(Toy::new(66, Mode::Main).with_terrain_panicking_at(t), unlit);
}

#[test]
fn starts_that_panic_fail_exactly_the_twelve_hundred_and_twenty_five_chunks_that_need_them() {
    let s = ChunkPos::new(-4, 3);
    let unlit: BTreeSet<_> = within(s, 20)
        .filter(|p| lit_depends(*p, Kind::Starts, s))
        .collect();
    assert_eq!(unlit.len(), 1225);
    fails_exactly(Toy::new(67, Mode::Main).with_starts_panicking_at(s), unlit);
}
